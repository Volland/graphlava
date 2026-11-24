//! The database handle, the process-global path registry and the writer
//! thread ([[16-api#Node.js binding]], designs D2 and D5).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use graphlava::{Database, OpenOptions, Perm, Reader, WriteRequest};
use napi::Env;
use napi::bindgen_prelude::{BigInt, Float64Array, Object};
use napi_derive::napi;

use crate::conv::{self, AdminIn, OpIn, OpenIn, Out, ReadIn, ids_out, op_result, query_result_out};
use crate::error::{Fail, R};
use crate::pool::{Job, guarded, on_commit_pool, on_pool, promise};
use crate::snapshot::{NativeRows, NativeSnapshot, Snap};

// ---- registry -----------------------------------------------------------------------

fn registry() -> &'static Mutex<HashSet<PathBuf>> {
    static REG: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The registry entry of one open file; removed when dropped, after the
/// database (field order in [`Owned`]).
#[derive(Debug)]
struct RegGuard(PathBuf);

impl Drop for RegGuard {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// The canonical form of a path whose file may not exist yet.
fn canonical(path: &Path) -> R<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| Fail::from(graphlava::Error::Io(e)))?
            .join(path)
    };
    if let Ok(c) = std::fs::canonicalize(&abs) {
        return Ok(c);
    }
    let name = abs
        .file_name()
        .ok_or_else(|| Fail::invalid(format!("not a file path: {}", path.display())))?;
    let dir = abs.parent().unwrap_or(Path::new("/"));
    let dir = std::fs::canonicalize(dir).map_err(|e| Fail::from(graphlava::Error::Io(e)))?;
    Ok(dir.join(name))
}

// @tg: implements:: [[openspec:node-binding#Single owner across workers and processes]]
/// An open database owned by this process, with its registry entry.
#[derive(Debug)]
pub(crate) struct Owned {
    // Field order matters: `db` closes (and releases the OS lock) before
    // `_reg` frees the path for the next `open` in this process.
    pub db: Database,
    /// Live handles that borrow `db`: snapshots, row sets, write transactions.
    pub live: AtomicUsize,
    pub write_timeout: Duration,
    _reg: RegGuard,
}

impl Owned {
    /// Opens `path`, refusing a file this process already holds (`LOCKED`),
    /// from this thread or any `worker_thread`.
    fn open(path: &str, o: &OpenIn) -> R<Owned> {
        let key = canonical(Path::new(path))?;
        {
            let mut reg = registry().lock().unwrap_or_else(|p| p.into_inner());
            if !reg.insert(key.clone()) {
                return Err(Fail::new(
                    "LOCKED",
                    format!(
                        "{} is already open in this process (one handle per file; \
                         handles cannot be shared with worker threads)",
                        key.display()
                    ),
                ));
            }
        }
        let reg = RegGuard(key);
        let mut opts = OpenOptions::default();
        let ms = |v: f64| Duration::from_micros((v * 1000.0).max(0.0) as u64);
        if let Some(v) = o.cache_bytes {
            opts.cache_bytes = v as usize;
        }
        if let Some(v) = o.group_window_ms {
            opts.group_window = ms(v);
        }
        if let Some(v) = o.group_bytes {
            opts.group_bytes = v as usize;
        }
        if let Some(v) = o.write_timeout_ms {
            opts.write_timeout = ms(v);
        }
        if let Some(v) = o.cascade_limit {
            opts.cascade_limit = v as u64;
        }
        if let Some(v) = o.graph_depth_limit {
            opts.graph_depth_limit = v;
        }
        if let Some(v) = o.max_relaxed_run {
            opts.max_relaxed_run = v as u64;
        }
        let write_timeout = opts.write_timeout;
        let db = Database::open(path, opts)?;
        Ok(Owned {
            db,
            live: AtomicUsize::new(0),
            write_timeout,
            _reg: reg,
        })
    }

    fn close(self) -> R<()> {
        let Owned { db, _reg, .. } = self;
        let r = db.close();
        drop(_reg);
        Ok(r?)
    }
}

/// Counts a live handle of an [`Owned`] while it exists.
#[derive(Debug)]
pub(crate) struct Live(Arc<Owned>);

impl Live {
    pub fn new(o: &Arc<Owned>) -> Live {
        o.live.fetch_add(1, Ordering::SeqCst);
        Live(Arc::clone(o))
    }
    pub fn owned(&self) -> &Arc<Owned> {
        &self.0
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

type Slot = Arc<Mutex<Option<Arc<Owned>>>>;

/// The native database handle behind the JS `Database`.
#[napi]
#[derive(Debug)]
pub struct NativeDb {
    slot: Slot,
    writer: Mutex<Option<Sender<Job>>>,
}

fn spawn_writer() -> Sender<Job> {
    let (tx, rx) = channel::<Job>();
    std::thread::Builder::new()
        .name("graphlava-writer".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            }
        })
        .expect("spawn writer thread");
    tx
}

impl NativeDb {
    fn new(o: Owned) -> NativeDb {
        NativeDb {
            slot: Arc::new(Mutex::new(Some(Arc::new(o)))),
            writer: Mutex::new(Some(spawn_writer())),
        }
    }

    pub(crate) fn owned(&self) -> R<Arc<Owned>> {
        self.slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(Fail::closed)
    }

    /// Runs `f` on the writer thread, in call order, and returns its Promise.
    fn on_writer<'e>(
        &self,
        env: &'e Env,
        f: impl FnOnce(Arc<Owned>) -> R<Out> + Send + 'static,
    ) -> napi::Result<Object<'e>> {
        let (settle, p) = promise(env)?;
        let owned = match self.owned() {
            Ok(o) => o,
            Err(e) => {
                settle.settle(Err(e));
                return Ok(p);
            }
        };
        let job: Job = Box::new(move || settle.settle(guarded(|| f(owned))));
        self.send_writer(job);
        Ok(p)
    }

    /// Runs a one-shot write request on the commit pool: requests from
    /// concurrent callers wait for their group commit together.
    fn on_commit<'e>(
        &self,
        env: &'e Env,
        f: impl FnOnce(Arc<Owned>) -> R<Out> + Send + 'static,
    ) -> napi::Result<Object<'e>> {
        let o = self.owned();
        on_commit_pool(env, move || f(o?))
    }

    fn send_writer(&self, job: Job) {
        let w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        match w.as_ref().map(|tx| tx.send(job)) {
            Some(Ok(())) => {}
            Some(Err(e)) => (e.0)(),
            None => drop(w),
        }
    }
}

impl Drop for NativeDb {
    /// The finalizer: a handle collected without `close()` hands its
    /// database to the writer thread, which drops it after any queued write;
    /// dropping flushes a pending group commit and releases the lock.
    fn drop(&mut self) {
        let owned = self.slot.lock().unwrap_or_else(|p| p.into_inner()).take();
        let tx = self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(o) = owned {
            let job: Job = Box::new(move || drop(o));
            match tx.as_ref().map(|tx| tx.send(job)) {
                Some(Ok(())) => {}
                Some(Err(e)) => (e.0)(),
                None => {}
            }
        }
    }
}

pub(crate) fn submit_ops(o: &Owned, ops: Vec<OpIn>, durability: &str) -> R<Out> {
    let d = conv::durability(durability)?;
    let ops = ops.into_iter().map(conv::core_op).collect::<R<Vec<_>>>()?;
    let t = o.db.submit(WriteRequest { ops }, d)?;
    Ok(Out::Obj(vec![
        (
            "results",
            Out::Arr(t.results.iter().map(op_result).collect()),
        ),
        ("batch", Out::Id(t.batch)),
    ]))
}

fn update_out(r: graphlava::UpdateReport) -> Out {
    Out::Obj(vec![
        ("inserted", Out::Num(r.inserted as f64)),
        ("deleted", Out::Num(r.deleted as f64)),
        ("updatedInPlace", Out::Num(r.updated_in_place as f64)),
    ])
}

fn perm(p: &str) -> R<Perm> {
    Ok(match p {
        "spo" => Perm::Spo,
        "pos" => Perm::Pos,
        "osp" => Perm::Osp,
        "pso" => Perm::Pso,
        "sop" => Perm::Sop,
        "ops" => Perm::Ops,
        _ => return Err(Fail::invalid(format!("unknown ordering `{p}`"))),
    })
}

fn perm_name(p: Perm) -> &'static str {
    match p {
        Perm::Spo => "spo",
        Perm::Pos => "pos",
        Perm::Osp => "osp",
        Perm::Pso => "pso",
        Perm::Sop => "sop",
        Perm::Ops => "ops",
    }
}

fn admin_call(o: &Owned, a: AdminIn) -> R<Out> {
    let admin = o.db.admin();
    Ok(match a.f.as_str() {
        "sweepDue" => Out::Bool(admin.sweep_due()?),
        "sweep" => {
            let w = Duration::from_micros((a.window_ms.unwrap_or(0.0) * 1000.0).max(0.0) as u64);
            let r = admin.sweep(w)?;
            Out::Obj(vec![
                ("marked", Out::Num(r.marked as f64)),
                ("examined", Out::Num(r.examined as f64)),
                ("deleted", Out::Num(r.deleted as f64)),
                ("keptYoung", Out::Num(r.kept_young as f64)),
                ("keptRereferenced", Out::Num(r.kept_rereferenced as f64)),
            ])
        }
        "buildOrdering" => {
            let p = a
                .perm
                .ok_or_else(|| Fail::invalid("buildOrdering: missing `perm`"))?;
            admin.build_ordering(perm(&p)?)?;
            Out::Undefined
        }
        "buildWantedOrderings" => {
            let built = admin.build_wanted_orderings(a.threshold.unwrap_or(0.0) as u64)?;
            Out::Arr(
                built
                    .into_iter()
                    .map(|p| Out::Str(perm_name(p).into()))
                    .collect(),
            )
        }
        "buildValueIndex" => {
            let p =
                a.p.ok_or_else(|| Fail::invalid("buildValueIndex: missing `p`"))?;
            admin.build_value_index(&conv::term(p, "p")?)?;
            Out::Undefined
        }
        "fileSize" => Out::Num(o.db.file_size()? as f64),
        "planCacheStats" => {
            let (h, m, i) = o.db.plan_cache_stats();
            Out::Obj(vec![
                ("hits", Out::Num(h as f64)),
                ("misses", Out::Num(m as f64)),
                ("invalidations", Out::Num(i as f64)),
            ])
        }
        f => return Err(Fail::invalid(format!("unknown admin call `{f}`"))),
    })
}

/// A core read on any reader view.
pub(crate) fn read_call<V: Reader + ?Sized>(r: &V, q: &ReadIn) -> R<Out> {
    let id = |v: &Option<BigInt>, what: &str| -> R<graphlava::Id> {
        conv::id_of(
            v.as_ref()
                .ok_or_else(|| Fail::invalid(format!("{}: missing `{what}`", q.f)))?,
            what,
        )
    };
    let opt = |v: &Option<BigInt>, what: &str| -> R<Option<graphlava::Id>> {
        v.as_ref().map(|b| conv::id_of(b, what)).transpose()
    };
    Ok(match q.f.as_str() {
        "props" => conv::matches_out(&r.props(id(&q.id, "node")?)?),
        "out" => conv::matches_out(&r.out(id(&q.s, "s")?, opt(&q.p, "p")?)?),
        "inc" => conv::matches_out(&r.inc(id(&q.o, "o")?, opt(&q.p, "p")?)?),
        "find" => conv::matches_out(&r.find(opt(&q.s, "s")?, opt(&q.p, "p")?, opt(&q.o, "o")?)?),
        "stmt" => match r.stmt(id(&q.id, "id")?)? {
            Some(t) => conv::triple_out(&t),
            None => Out::Null,
        },
        "isUnasserted" => Out::Bool(r.is_unasserted(id(&q.id, "id")?)?),
        "hedge" => match r.hedge(id(&q.id, "id")?)? {
            Some(h) => conv::hedge_out(&h),
            None => Out::Null,
        },
        "lookup" => {
            let t = q
                .term
                .as_ref()
                .ok_or_else(|| Fail::invalid("lookup: missing term"))?;
            let t = conv::term(clone_term(t), "term")?;
            conv::opt_id(r.lookup(&t)?)
        }
        "graph" => {
            let t = q
                .term
                .as_ref()
                .ok_or_else(|| Fail::invalid("graph: missing name"))?;
            let t = conv::term(clone_term(t), "name")?;
            conv::opt_id(r.graph(&t)?)
        }
        "decode" => {
            let i = id(&q.id, "id")?;
            if i.0 == 0 {
                Out::Undefined
            } else {
                conv::term_out(&r.decode(i)?)
            }
        }
        "decodeMany" => {
            let ids = q
                .ids
                .as_ref()
                .ok_or_else(|| Fail::invalid("decodeMany: missing ids"))?;
            let mut out = Vec::with_capacity(ids.len());
            for i in ids {
                let i = conv::id_of(i, "ids")?;
                out.push(if i.0 == 0 {
                    Out::Undefined
                } else {
                    conv::term_out(&r.decode(i)?)
                });
            }
            Out::Arr(out)
        }
        "homeGraph" => conv::id_out(r.home_graph(id(&q.id, "id")?)?),
        "graphsOf" => ids_out(&r.graphs_of(id(&q.id, "id")?)?),
        "parents" => ids_out(&r.parents(id(&q.id, "graph")?)?),
        "children" => ids_out(&r.children(id(&q.id, "graph")?)?),
        "ancestors" => ids_out(&r.ancestors(id(&q.id, "graph")?)?),
        "descendants" => ids_out(&r.descendants(id(&q.id, "graph")?)?),
        "scopeStatements" => {
            ids_out(&r.scope_statements(id(&q.id, "graph")?, q.direct.unwrap_or(false))?)
        }
        "hedgesOfType" => ids_out(&r.hedges_of_type(id(&q.id, "type")?, None)?),
        "commitCounter" => Out::Num(r.commit_counter()? as f64),
        "checkInvariants" => match r.check_invariants()? {
            Some(v) => Out::Str(v),
            None => Out::Null,
        },
        f => return Err(Fail::invalid(format!("unknown read `{f}`"))),
    })
}

fn clone_term(t: &conv::TermIn) -> conv::TermIn {
    conv::TermIn {
        k: t.k.clone(),
        v: t.v.clone(),
        dt: t.dt.clone(),
        lang: t.lang.clone(),
        id: t.id.clone(),
    }
}

// ---- JS surface -----------------------------------------------------------------------

/// `open(path, options)`: resolves with a [`NativeDb`] or rejects (`LOCKED`, ...).
#[napi(ts_return_type = "Promise<NativeDb>")]
pub fn open<'e>(env: &'e Env, path: String, opts: Option<OpenIn>) -> napi::Result<Object<'e>> {
    let opts = opts.unwrap_or_default();
    on_pool(env, move || {
        Ok(Out::Db(NativeDb::new(Owned::open(&path, &opts)?)))
    })
}

/// The blocking `open` of `@graphlava/node/sync`.
#[napi]
pub fn open_sync(path: String, opts: Option<OpenIn>) -> napi::Result<NativeDb> {
    let opts = opts.unwrap_or_default();
    Ok(guarded(|| Owned::open(&path, &opts)).map(NativeDb::new)?)
}

/// Closes `slot`'s database on the current thread; `BUSY` while snapshots,
/// row sets or write transactions are alive.
fn close_slot(slot: &Slot) -> R<()> {
    let Some(mut arc) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() else {
        return Ok(());
    };
    loop {
        if arc.live.load(Ordering::SeqCst) > 0 {
            let n = arc.live.load(Ordering::SeqCst);
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(arc);
            return Err(Fail::new(
                "BUSY",
                format!(
                    "close() while {n} snapshot(s), row set(s) or write transaction(s) are open; \
                     finish or close them first"
                ),
            ));
        }
        match Arc::try_unwrap(arc) {
            Ok(o) => return o.close(),
            // An in-flight call still holds the database; it ends shortly.
            Err(a) => {
                arc = a;
                std::thread::sleep(Duration::from_micros(200));
            }
        }
    }
}

#[napi]
impl NativeDb {
    /// In-memory metadata: whether the handle is open.
    #[napi]
    pub fn is_open(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub fn close<'e>(&self, env: &'e Env) -> napi::Result<Object<'e>> {
        let (settle, p) = promise(env)?;
        let slot = Arc::clone(&self.slot);
        // On the writer thread, after every write queued before it.
        let job: Job =
            Box::new(move || settle.settle(guarded(|| close_slot(&slot).map(|_| Out::Undefined))));
        self.send_writer(job);
        Ok(p)
    }

    #[napi]
    pub fn close_sync(&self) -> napi::Result<()> {
        Ok(guarded(|| close_slot(&self.slot))?)
    }

    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn submit<'e>(
        &self,
        env: &'e Env,
        ops: Vec<OpIn>,
        durability: String,
    ) -> napi::Result<Object<'e>> {
        self.on_commit(env, move |o| submit_ops(&o, ops, &durability))
    }

    #[napi]
    pub fn submit_sync(&self, ops: Vec<OpIn>, durability: String) -> napi::Result<Out> {
        let o = self.owned()?;
        Ok(guarded(|| submit_ops(&o, ops, &durability))?)
    }

    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn sparql_update<'e>(
        &self,
        env: &'e Env,
        text: String,
        durability: String,
    ) -> napi::Result<Object<'e>> {
        self.on_commit(env, move |o| {
            let d = conv::durability(&durability)?;
            Ok(update_out(o.db.sparql_update(&text, d)?))
        })
    }

    #[napi]
    pub fn sparql_update_sync(&self, text: String, durability: String) -> napi::Result<Out> {
        let o = self.owned()?;
        Ok(guarded(|| {
            let d = conv::durability(&durability)?;
            Ok(update_out(o.db.sparql_update(&text, d)?))
        })?)
    }

    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn datalog_update<'e>(
        &self,
        env: &'e Env,
        program: String,
        durability: String,
    ) -> napi::Result<Object<'e>> {
        self.on_commit(env, move |o| {
            let d = conv::durability(&durability)?;
            Ok(query_result_out(&o.db.datalog_update(&program, d)?))
        })
    }

    #[napi]
    pub fn datalog_update_sync(&self, program: String, durability: String) -> napi::Result<Out> {
        let o = self.owned()?;
        Ok(guarded(|| {
            let d = conv::durability(&durability)?;
            Ok(query_result_out(&o.db.datalog_update(&program, d)?))
        })?)
    }

    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn admin<'e>(&self, env: &'e Env, req: AdminIn) -> napi::Result<Object<'e>> {
        self.on_writer(env, move |o| admin_call(&o, req))
    }

    #[napi]
    pub fn admin_sync(&self, req: AdminIn) -> napi::Result<Out> {
        let o = self.owned()?;
        Ok(guarded(|| admin_call(&o, req))?)
    }

    /// Runs a query on a fresh snapshot; resolves with a [`NativeRows`].
    #[napi(ts_return_type = "Promise<NativeRows>")]
    pub fn query<'e>(&self, env: &'e Env, lang: String, text: String) -> napi::Result<Object<'e>> {
        let o = self.owned();
        on_pool(env, move || {
            let snap = Snap::new(&o?)?;
            Ok(Out::Rows(NativeRows::run(snap, &lang, &text)?))
        })
    }

    #[napi]
    pub fn query_sync(&self, lang: String, text: String) -> napi::Result<NativeRows> {
        let o = self.owned()?;
        Ok(guarded(|| NativeRows::run(Snap::new(&o)?, &lang, &text))?)
    }

    /// Opens a read snapshot; resolves with a [`NativeSnapshot`].
    #[napi(ts_return_type = "Promise<NativeSnapshot>")]
    pub fn snapshot<'e>(&self, env: &'e Env) -> napi::Result<Object<'e>> {
        let o = self.owned();
        on_pool(env, move || {
            Ok(Out::Snap(NativeSnapshot::new(Snap::new(&o?)?)))
        })
    }

    #[napi]
    pub fn snapshot_sync(&self) -> napi::Result<NativeSnapshot> {
        let o = self.owned()?;
        Ok(guarded(|| Snap::new(&o).map(NativeSnapshot::new))?)
    }

    /// One core read on a fresh snapshot.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn read<'e>(&self, env: &'e Env, req: ReadIn) -> napi::Result<Object<'e>> {
        let o = self.owned();
        on_pool(env, move || {
            let o = o?;
            let r = o.db.read()?;
            read_call(&r, &req)
        })
    }

    #[napi]
    pub fn read_sync(&self, req: ReadIn) -> napi::Result<Out> {
        let o = self.owned()?;
        Ok(guarded(|| read_call(&o.db.read()?, &req))?)
    }

    /// Starts an interactive write transaction on the writer thread.
    #[napi(ts_return_type = "Promise<NativeTxn>")]
    pub fn begin_write<'e>(&self, env: &'e Env, durability: String) -> napi::Result<Object<'e>> {
        let (settle, p) = promise(env)?;
        let d = match conv::durability(&durability).and_then(|d| Ok((d, self.owned()?))) {
            Ok(v) => v,
            Err(e) => {
                settle.settle(Err(e));
                return Ok(p);
            }
        };
        let job: Job = Box::new(move || crate::txn::run_txn(d.1, d.0, settle));
        self.send_writer(job);
        Ok(p)
    }

    // @tg: implements:: [[openspec:node-binding#Node call overhead]]
    /// Benchmark hook: `n` timed `props(node)` calls through the Rust API on
    /// one snapshot each, in nanoseconds ([[17-benchmarks#Binding overhead]]).
    #[napi]
    pub fn bench_rust_props(&self, node: BigInt, n: u32) -> napi::Result<Float64Array> {
        let o = self.owned()?;
        let id = conv::id_of(&node, "node")?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let t = std::time::Instant::now();
            let r = o.db.read().map_err(Fail::from)?;
            std::hint::black_box(r.props(id).map_err(Fail::from)?);
            out.push(t.elapsed().as_nanos() as f64);
        }
        Ok(Float64Array::new(out))
    }
}
