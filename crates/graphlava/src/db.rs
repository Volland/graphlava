//! The public database: one owner process, snapshot reads, one writer, three
//! durability levels and batched group commit ([[06-transactions-durability]],
//! [[16-api]], [[adr:0012]], [[adr:0025]], [[adr:0026]]).
//!
//! Writer design: a mutex guards the open batch (one engine write transaction)
//! and the writer token. A request (one-shot [`WriteRequest`] or interactive
//! [`WriteTxn`]) holds the token, runs against an overlay on the batch, and is
//! merged into the batch only when it succeeds. A batch commits once: with
//! `Flush` when it holds a `sync` or `ordered` request, else with `NoFlush`.
//! A flusher thread closes `ordered` batches when their window expires.

// @tg: implements:: [[openspec:transactions#Per-transaction durability levels]]
// @tg: implements:: [[openspec:transactions#Batched commit]]
// @tg: implements:: [[openspec:transactions#One-shot remote writes]]
// @tg: implements:: [[openspec:transactions#Atomic transactions]]
// @tg: implements:: [[openspec:transactions#Portable file format]]

use std::collections::HashSet;
use std::ops::Bound;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::dict::{Dict, DictConfig, Record};
use crate::error::{Error, Result};
use crate::hedge::{self, HedgeType, HedgeTypeDecl, Hyperedge};
use crate::id::{self, DEFAULT_GRAPH, Id, Kind, SYSTEM_GRAPH};
use crate::sparql::update::UpdateReport;
use crate::sparql::{QueryOptions, QueryResult};
use crate::storage::fault::FaultPlan;
use crate::storage::overlay::{Overlay, WriteSet};
use crate::storage::redb_engine::{EngineOptions, RedbRead, RedbStorage, RedbWrite};
use crate::storage::{EngineDurability, KvRead, KvWrite, RangeIter, Storage};
use crate::store::{self, DeleteMode, Match, PredicateDecl, Store, StoreConfig};
use crate::tables::{self as tb, Perm, feature, mk};
use crate::term::{Term, TermRef, Triple};

/// Durability level of one write ([[06-transactions-durability#Durability levels]]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    /// Survives any crash when the call returns; closes the batch at once.
    Sync,
    /// Survives any crash when the call returns; shares a group commit.
    #[default]
    Ordered,
    /// Atomic and crash-consistent; may be lost until a later durable commit.
    Relaxed,
}

impl Durability {
    fn durable(self) -> bool {
        !matches!(self, Durability::Relaxed)
    }
}

/// Settings for [`Database::open`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub cache_bytes: usize,
    /// Group-commit window N (rule `tx-group-window`).
    pub group_window: Duration,
    /// Group-commit size limit M.
    pub group_bytes: usize,
    /// Time limit of an interactive write transaction (rule `api-write-in-process`).
    pub write_timeout: Duration,
    pub cascade_limit: u64,
    /// Fixed per file at creation (rule `dict-owned-threshold`).
    pub owned_threshold: usize,
    /// Fixed per file at creation (rule `dict-ns-cap`).
    pub ns_cap: u32,
    /// Quick repair (rule `tx-quick-repair`): durable commits also save the
    /// allocator state, so opening after a crash takes milliseconds instead
    /// of a full repair (seconds at 10M statements, minutes at 100M), at
    /// about twice the durable-commit latency. Default: on for iOS and
    /// Android, off elsewhere.
    pub quick_repair: bool,
    /// At most this many non-durable batch commits in a row; the next one is
    /// durable, so redb can free pages and relaxed data has a bounded loss
    /// window (rule `tx-relaxed-bound`).
    pub max_relaxed_run: u64,
    /// Graph nesting depth limit, fixed per file at creation (rule `gr-dag-depth`).
    pub graph_depth_limit: u32,
    /// Crash-harness hook; never set in production.
    #[doc(hidden)]
    pub fault: Option<FaultPlan>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            cache_bytes: 64 << 20,
            group_window: Duration::from_millis(2),
            group_bytes: 4 << 20,
            write_timeout: Duration::from_millis(100),
            cascade_limit: 100_000,
            owned_threshold: 256,
            ns_cap: 65_536,
            graph_depth_limit: crate::graph::DEFAULT_DEPTH_LIMIT,
            max_relaxed_run: 100,
            quick_repair: cfg!(any(target_os = "ios", target_os = "android")),
            fault: None,
        }
    }
}

/// One operation of a one-shot write request ([[16-api#Core API]]).
#[derive(Debug, Clone)]
pub enum CoreOp {
    /// Insert into `graph` (an IRI or graph ID; a new name creates the graph),
    /// or without a graph into the subject's home graph.
    Insert {
        s: TermRef,
        p: TermRef,
        o: TermRef,
        graph: Option<TermRef>,
    },
    Set {
        s: TermRef,
        p: TermRef,
        o: TermRef,
    },
    Update {
        id: Id,
        o: TermRef,
        expected: Option<TermRef>,
    },
    Annotate {
        id: Id,
        p: TermRef,
        o: TermRef,
    },
    Delete {
        id: Id,
        mode: DeleteMode,
    },
    /// Remove a triple; inside a graph, only its membership there.
    Remove {
        s: TermRef,
        p: TermRef,
        o: TermRef,
        mode: DeleteMode,
        graph: Option<TermRef>,
    },
    /// The statement ID of an existing triple, promoting it if needed.
    StatementId {
        s: TermRef,
        p: TermRef,
        o: TermRef,
    },
    InsertUnasserted {
        s: TermRef,
        p: TermRef,
        o: TermRef,
    },
    DeclarePredicate {
        p: TermRef,
        decl: PredicateDecl,
    },
    /// Creates a named graph (or returns the existing one).
    CreateGraph {
        name: TermRef,
    },
    AddMember {
        item: Id,
        graph: TermRef,
    },
    RemoveMember {
        item: Id,
        graph: TermRef,
    },
    MoveMember {
        item: Id,
        from: TermRef,
        to: TermRef,
    },
    /// The statement ID of membership `(item, :in, graph)`, promoting it.
    MembershipId {
        item: Id,
        graph: TermRef,
    },
    Nest {
        child: TermRef,
        parent: TermRef,
    },
    Unnest {
        child: TermRef,
        parent: TermRef,
    },
    DropGraph {
        graph: TermRef,
        recursive: bool,
    },
    DeclareHedgeType {
        ty: TermRef,
        decl: HedgeTypeDecl,
    },
    CreateHedge {
        ty: TermRef,
        graph: Option<TermRef>,
        roles: Vec<(TermRef, TermRef)>,
        attrs: Vec<(TermRef, TermRef)>,
    },
    UpdateRole {
        hedge: Id,
        role: TermRef,
        filler: TermRef,
    },
    /// The statement ID of role edge `(hedge, role, node)`, promoting it.
    RoleId {
        hedge: Id,
        role: TermRef,
        node: TermRef,
    },
}

/// A complete write request from outside an interactive transaction
/// ([[16-api#Write requests]]): all of its operations apply, or none.
#[derive(Debug, Clone, Default)]
pub struct WriteRequest {
    pub ops: Vec<CoreOp>,
}

/// The result of one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpResult {
    /// The statement ID, or `None` for an unpromoted inline property.
    Id(Option<Id>),
    Deleted(Vec<Id>),
    Removed(bool),
    /// Whether the operation changed anything (memberships, nesting).
    Changed(bool),
    Done,
}

/// The content of a complete write request ([[16-api#Write requests]]).
#[derive(Debug, Clone)]
pub enum RequestBody {
    /// Core operations, applied in order, all or nothing.
    Ops(WriteRequest),
    /// A SPARQL 1.2 Update.
    SparqlUpdate(String),
    /// A Datalog program whose `@assert` heads write.
    Datalog(String),
}

fn once_key(key: &str) -> String {
    format!("journal_seq:{key}")
}

fn suspend_aborted() -> Error {
    Error::Aborted("the database was suspended while this write transaction was open".into())
}

/// Returned once the request's durability level holds.
#[derive(Debug, Clone)]
pub struct CommitTicket {
    pub results: Vec<OpResult>,
    /// Sequence number of the batch the request committed in.
    pub batch: u64,
}

fn run_op(st: &mut Store<'_>, op: &CoreOp) -> Result<OpResult> {
    Ok(match op {
        CoreOp::Insert { s, p, o, graph } => OpResult::Id(st.insert_in(graph.as_ref(), s, p, o)?),
        CoreOp::Set { s, p, o } => OpResult::Id(st.set(s, p, o)?),
        CoreOp::Update { id, o, expected } => {
            st.update(*id, o, expected.as_ref())?;
            OpResult::Done
        }
        CoreOp::Annotate { id, p, o } => OpResult::Id(st.annotate(*id, p, o)?),
        CoreOp::Delete { id, mode } => OpResult::Deleted(st.delete(*id, *mode)?),
        CoreOp::Remove {
            s,
            p,
            o,
            mode,
            graph,
        } => OpResult::Removed(st.remove_in(graph.as_ref(), s, p, o, *mode)?),
        CoreOp::StatementId { s, p, o } => OpResult::Id(st.statement_id(s, p, o)?),
        CoreOp::InsertUnasserted { s, p, o } => OpResult::Id(Some(st.insert_unasserted(s, p, o)?)),
        CoreOp::DeclarePredicate { p, decl } => {
            st.declare_predicate(p, *decl)?;
            OpResult::Done
        }
        CoreOp::CreateGraph { name } => OpResult::Id(Some(st.graph(name, true)?)),
        CoreOp::AddMember { item, graph } => {
            let g = st.graph(graph, true)?;
            OpResult::Changed(st.add_member(*item, g)?)
        }
        CoreOp::RemoveMember { item, graph } => {
            let g = st.graph(graph, false)?;
            OpResult::Deleted(st.remove_member(*item, g)?)
        }
        CoreOp::MoveMember { item, from, to } => {
            let from = st.graph(from, false)?;
            let to = st.graph(to, true)?;
            OpResult::Deleted(st.move_member(*item, from, to)?)
        }
        CoreOp::MembershipId { item, graph } => {
            let g = st.graph(graph, false)?;
            OpResult::Id(Some(st.membership_id(*item, g)?))
        }
        CoreOp::Nest { child, parent } => {
            let (c, p) = (st.graph(child, true)?, st.graph(parent, true)?);
            OpResult::Changed(st.nest(c, p)?)
        }
        CoreOp::Unnest { child, parent } => {
            let (c, p) = (st.graph(child, false)?, st.graph(parent, false)?);
            OpResult::Changed(st.unnest(c, p)?)
        }
        CoreOp::DropGraph { graph, recursive } => {
            let g = st.graph(graph, false)?;
            OpResult::Deleted(st.drop_graph(g, *recursive)?)
        }
        CoreOp::DeclareHedgeType { ty, decl } => {
            OpResult::Id(Some(st.declare_hedge_type(ty, decl)?))
        }
        CoreOp::CreateHedge {
            ty,
            graph,
            roles,
            attrs,
        } => OpResult::Id(Some(st.create_hedge(ty, graph.as_ref(), roles, attrs)?)),
        CoreOp::UpdateRole {
            hedge,
            role,
            filler,
        } => {
            st.update_role(*hedge, role, filler)?;
            OpResult::Done
        }
        CoreOp::RoleId { hedge, role, node } => {
            let r = st.intern(role)?;
            let n = st.intern(node)?;
            OpResult::Id(Some(st.role_statement(*hedge, r, n)?))
        }
    })
}

// ---- writer state -----------------------------------------------------------------

struct Batch {
    w: RedbWrite,
    opened: Instant,
    durable: bool,
    bytes: usize,
    seq: u64,
}

struct WState {
    batch: Option<Batch>,
    /// Sequence number the next batch will get.
    next_seq: u64,
    /// Highest committed batch sequence number.
    committed: u64,
    /// After a failed batch commit the database is read-only (rule `tx-group-ack`).
    failed_from: Option<u64>,
    /// The writer token: (ticket, deadline) of the current interactive writer.
    owner: Option<(u64, Option<Instant>)>,
    next_ticket: u64,
    waiting: usize,
    shutdown: bool,
    last_clock: Option<Instant>,
    /// Non-durable batch commits since the last durable one.
    nondurable_run: u64,
    /// Durable batch commits so far (diagnostics).
    durable_commits: u64,
    /// Set by [`Database::suspend`]: writers wait in `acquire` until resume.
    suspended: bool,
    /// The interactive writer whose token suspend revoked; its next call
    /// fails with [`Error::Aborted`].
    revoked: Option<u64>,
}

/// The reader gate of suspend and resume: new snapshots wait while
/// `suspended`, and `snapshots` counts the live ones so the file lock is
/// released only when none is open.
#[derive(Debug, Default)]
struct Gate {
    suspended: bool,
    snapshots: usize,
}

/// How long suspend waits for open snapshots to end before it gives up on
/// releasing the file lock.
const SUSPEND_SNAPSHOT_WAIT: Duration = Duration::from_secs(1);

struct Inner {
    storage: RedbStorage,
    dict: Dict,
    opts: OpenOptions,
    w: Mutex<WState>,
    cv: Condvar,
    /// The sweep's kept set while a sweep runs ([[03-dictionary#Mark and sweep]]).
    kept: Mutex<Option<HashSet<Id>>>,
    /// Plan cache shared by SPARQL queries (rule `plan-one-pipeline`).
    plans: crate::query::plan::PlanCache<crate::sparql::Planned>,
    /// Misses on on-demand orderings (PSO, SOP, OPS) counted by plans.
    wanted: [std::sync::atomic::AtomicU64; 3],
    gate: Mutex<Gate>,
    gate_cv: Condvar,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, WState> {
        self.w.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Counts a new snapshot, waiting while the database is suspended.
    fn enter_snapshot(&self) {
        let mut g = self.gate();
        while g.suspended {
            g = self.gate_cv.wait(g).unwrap_or_else(|p| p.into_inner());
        }
        g.snapshots += 1;
    }

    fn leave_snapshot(&self) {
        let mut g = self.gate();
        g.snapshots -= 1;
        self.gate_cv.notify_all();
    }

    fn store_cfg(&self) -> StoreConfig {
        StoreConfig {
            cascade_limit: self.opts.cascade_limit,
        }
    }

    fn ensure_batch(&self, st: &mut WState) -> Result<()> {
        if st.failed_from.is_some() {
            return Err(Error::ReadOnly);
        }
        if st.batch.is_none() {
            let w = self.storage.begin_write()?;
            let seq = st.next_seq;
            st.next_seq += 1;
            st.batch = Some(Batch {
                w,
                opened: Instant::now(),
                durable: false,
                bytes: 0,
                seq,
            });
        }
        Ok(())
    }

    /// Commits the open batch (rule `tx-group-one-txn`).
    fn close_batch(&self, st: &mut WState) {
        let Some(mut b) = st.batch.take() else { return };
        let res = (|| -> Result<()> {
            let c = tb::meta_u64(&b.w, mk::COMMIT)?.unwrap_or(0) + 1;
            tb::meta_put_u64(&mut b.w, mk::COMMIT, c)?;
            if st
                .last_clock
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
            {
                b.w.put(&tb::TXCLOCK, &c.to_be_bytes(), &unix_micros().to_le_bytes())?;
                st.last_clock = Some(Instant::now());
            }
            // redb frees pages only at durable commits, so a run of
            // non-durable ones is bounded (engine bake-off, recommendation 1).
            if !b.durable && st.nondurable_run + 1 >= self.opts.max_relaxed_run.max(1) {
                b.durable = true;
            }
            let d = if b.durable {
                st.nondurable_run = 0;
                st.durable_commits += 1;
                EngineDurability::Flush
            } else {
                st.nondurable_run += 1;
                EngineDurability::NoFlush
            };
            self.storage.commit(b.w, d)
        })();
        match res {
            Ok(()) => st.committed = b.seq,
            Err(_) => st.failed_from = Some(b.seq),
        }
        self.cv.notify_all();
    }

    /// Makes every acknowledged commit durable: commits the open batch with a
    /// flush, or, when the last batches committed without one, a durable
    /// empty batch (a durable commit makes every earlier commit durable).
    fn flush_durable(&self, st: &mut WState) {
        if st.batch.is_none() && st.nondurable_run > 0 && self.ensure_batch(st).is_err() {
            return;
        }
        if let Some(b) = st.batch.as_mut() {
            b.durable = true;
        }
        self.close_batch(st);
    }

    /// Waits for the writer token. Interactive writers past their deadline are
    /// revoked so queued requests proceed (rule `api-write-in-process`).
    fn acquire<'a>(
        &'a self,
        mut st: MutexGuard<'a, WState>,
        deadline: Option<Instant>,
    ) -> (MutexGuard<'a, WState>, u64) {
        st.waiting += 1;
        loop {
            if st.suspended {
                // Writers wait for resume (rule `api-ios-suspend`).
                st = self.cv.wait(st).unwrap_or_else(|p| p.into_inner());
                continue;
            }
            match st.owner {
                None => break,
                Some((_, Some(d))) if Instant::now() >= d => {
                    st.owner = None;
                    break;
                }
                Some((_, d)) => {
                    let wait = d.map_or(Duration::from_millis(50), |d| {
                        d.saturating_duration_since(Instant::now())
                    });
                    st = self
                        .cv
                        .wait_timeout(st, wait.max(Duration::from_micros(100)))
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                }
            }
        }
        st.waiting -= 1;
        let t = st.next_ticket;
        st.next_ticket += 1;
        st.owner = Some((t, deadline));
        (st, t)
    }

    fn release(&self, st: &mut WState, ticket: u64) {
        if st.owner.is_some_and(|(t, _)| t == ticket) {
            st.owner = None;
        }
        self.cv.notify_all();
    }

    fn note_interned(&self, ids: &[Id]) {
        if let Some(k) = self.kept.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            k.extend(ids.iter().copied());
        }
    }

    /// Merges a finished request into the batch and decides whether the batch
    /// closes now. Returns the batch sequence number.
    fn merge(&self, st: &mut WState, writes: WriteSet, durability: Durability) -> Result<u64> {
        self.ensure_batch(st)?;
        let b = st.batch.as_mut().expect("batch");
        b.bytes += writes.bytes;
        b.durable |= durability.durable();
        let seq = b.seq;
        if let Err(e) = writes.apply(&mut b.w) {
            // A half-applied request poisons the batch: every request in it fails.
            let b = st.batch.take().expect("batch");
            let _ = self.storage.abort(b.w);
            st.failed_from = Some(seq);
            self.cv.notify_all();
            return Err(e);
        }
        let b = st.batch.as_ref().expect("batch");
        let close = durability == Durability::Sync
            || b.bytes >= self.opts.group_bytes
            || (!b.durable && st.waiting == 0);
        if close {
            self.close_batch(st);
        } else {
            self.cv.notify_all(); // wake the flusher to arm its timer
        }
        Ok(seq)
    }

    /// Blocks until batch `seq` is committed (rule `tx-group-ack`).
    fn wait_durable<'a>(&'a self, mut st: MutexGuard<'a, WState>, seq: u64) -> Result<()> {
        loop {
            if st.committed >= seq {
                return Ok(());
            }
            if st.failed_from.is_some_and(|f| f <= seq) {
                return Err(Error::Io(std::io::Error::other(
                    "the group commit of this request failed",
                )));
            }
            st = self.cv.wait(st).unwrap_or_else(|p| p.into_inner());
        }
    }

    fn flusher(self: Arc<Self>) {
        let mut st = self.lock();
        loop {
            if st.shutdown {
                return;
            }
            let wait = match &st.batch {
                Some(b) if b.durable => {
                    let age = b.opened.elapsed();
                    if age >= self.opts.group_window {
                        self.close_batch(&mut st);
                        continue;
                    }
                    self.opts.group_window - age
                }
                // A relaxed-only batch left open by a waiting writer.
                Some(_) if st.owner.is_none() && st.waiting == 0 => {
                    self.close_batch(&mut st);
                    continue;
                }
                _ => Duration::from_millis(50),
            };
            st = self
                .cv
                .wait_timeout(st, wait)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Runs a closure as one request under the writer token.
    fn request<R>(
        &self,
        durability: Durability,
        f: impl FnOnce(&mut Store<'_>) -> Result<R>,
    ) -> Result<(R, u64)> {
        let (mut st, ticket) = self.acquire(self.lock(), None);
        let out: Result<(R, u64)> = (|| {
            self.ensure_batch(&mut st)?;
            let base = &st.batch.as_ref().expect("batch").w;
            let mut ov = Overlay::new(base);
            let mut store = Store::new(&mut ov, self.dict, self.store_cfg());
            let r = f(&mut store)?;
            let interned = std::mem::take(&mut store.interned);
            let writes = ov.into_writes();
            self.note_interned(&interned);
            let seq = self.merge(&mut st, writes, durability)?;
            Ok((r, seq))
        })();
        self.release(&mut st, ticket);
        let (r, seq) = out?;
        if durability.durable() {
            self.wait_durable(st, seq)?;
        }
        Ok((r, seq))
    }
}

/// The owner handle of one database file. `Send + Sync`.
#[derive(Debug)]
pub struct Database {
    inner: Arc<Inner>,
    flusher: Option<JoinHandle<()>>,
}

impl Database {
    /// Opens or creates a database file. Takes the OS lock on the file for the
    /// life of the handle; a second open anywhere fails with [`Error::Locked`]
    /// (rule `api-single-owner`). The format version and feature flags are
    /// checked before any other read (rule `tx-recovery-format`).
    pub fn open(path: impl AsRef<Path>, opts: OpenOptions) -> Result<Database> {
        let storage = RedbStorage::open(
            path.as_ref(),
            &EngineOptions {
                cache_bytes: opts.cache_bytes,
                quick_repair: opts.quick_repair,
                fault: opts.fault.clone(),
            },
        )?;
        let r = storage.begin_read()?;
        let dict_cfg = match tb::meta_u64(&r, mk::FORMAT)? {
            Some(v) => {
                if v != tb::FORMAT_VERSION {
                    return Err(Error::FormatMismatch(format!(
                        "format version {v}, this build reads {}",
                        tb::FORMAT_VERSION
                    )));
                }
                let f = tb::meta_u64(&r, mk::FEATURES)?.unwrap_or(0);
                if f & !feature::KNOWN != 0 {
                    return Err(Error::FormatMismatch(format!(
                        "unknown feature flags {:#x}",
                        f & !feature::KNOWN
                    )));
                }
                DictConfig {
                    owned_threshold: tb::meta_u64(&r, mk::OWNED_THRESHOLD)?.unwrap_or(256) as usize,
                    ns_cap: tb::meta_u64(&r, mk::NS_CAP)?.unwrap_or(65_536) as u32,
                }
            }
            None => {
                if !r
                    .scan(&tb::STMT, Bound::Unbounded, Bound::Unbounded, false, 1)?
                    .is_empty()
                    || !r
                        .scan(&tb::META, Bound::Unbounded, Bound::Unbounded, false, 1)?
                        .is_empty()
                {
                    return Err(Error::FormatMismatch("not a graphlava file".into()));
                }
                drop(r);
                let cfg = DictConfig {
                    owned_threshold: opts.owned_threshold,
                    ns_cap: opts.ns_cap,
                };
                Self::initialize(&storage, cfg, opts.graph_depth_limit)?;
                cfg
            }
        };
        let inner = Arc::new(Inner {
            storage,
            dict: Dict::new(dict_cfg),
            opts,
            w: Mutex::new(WState {
                batch: None,
                next_seq: 1,
                committed: 0,
                failed_from: None,
                owner: None,
                next_ticket: 1,
                waiting: 0,
                shutdown: false,
                last_clock: None,
                nondurable_run: 0,
                durable_commits: 0,
                suspended: false,
                revoked: None,
            }),
            cv: Condvar::new(),
            kept: Mutex::new(None),
            plans: crate::query::plan::PlanCache::new(),
            wanted: Default::default(),
            gate: Mutex::new(Gate::default()),
            gate_cv: Condvar::new(),
        });
        let fl = Arc::clone(&inner);
        let flusher = std::thread::Builder::new()
            .name("graphlava-flusher".into())
            .spawn(move || fl.flusher())?;
        Ok(Database {
            inner,
            flusher: Some(flusher),
        })
    }

    fn initialize(storage: &RedbStorage, cfg: DictConfig, depth_limit: u32) -> Result<()> {
        let mut w = storage.begin_write()?;
        tb::meta_put_u64(&mut w, mk::FORMAT, tb::FORMAT_VERSION)?;
        tb::meta_put_u64(&mut w, mk::FEATURES, 0)?;
        tb::meta_put_u64(&mut w, mk::COMMIT, 0)?;
        tb::meta_put_u64(&mut w, mk::NEXT_CHUNK, 1)?;
        tb::meta_put_u64(&mut w, mk::OWNED_THRESHOLD, cfg.owned_threshold as u64)?;
        tb::meta_put_u64(&mut w, mk::NS_CAP, cfg.ns_cap as u64)?;
        // Graph chunk 0 belongs to the system graph (rule `id-graph-bootstrap`);
        // offset 0 is the system graph, offset 1 the default graph ([[adr:0038]]).
        let mut g = [0u8; 16];
        g[0..8].copy_from_slice(&SYSTEM_GRAPH.le_bytes());
        g[8..16].copy_from_slice(&1u64.to_le_bytes());
        w.put(&tb::GCHUNK, &0u64.to_be_bytes(), &g)?;
        let mut kc = [0u8; 16];
        kc[0..8].copy_from_slice(&1u64.to_be_bytes());
        w.put(&tb::KIND_CHUNK, &kc, &[])?;
        let mut lease = [0u8; 16];
        lease[8..16].copy_from_slice(&(DEFAULT_GRAPH.offset() + 1).to_le_bytes());
        tb::meta_put(
            &mut w,
            &format!("lease:{:016x}:{:016x}", SYSTEM_GRAPH.raw(), 1u64),
            &lease,
        )?;
        // System IRIs used by virtual triples, interned with the file.
        let dict = Dict::new(cfg);
        let in_id = dict.intern_iri(&mut w, crate::term::META_IN)?;
        tb::meta_put_u64(&mut w, mk::SYS_IN, in_id.raw())?;
        let type_id = dict.intern_iri(&mut w, crate::term::RDF_TYPE)?;
        tb::meta_put_u64(&mut w, mk::SYS_TYPE, type_id.raw())?;
        tb::meta_put_u64(&mut w, mk::DEPTH_LIMIT, u64::from(depth_limit))?;
        storage.commit(w, EngineDurability::Flush)
    }

    /// A read snapshot (rule `tx-snapshot-read`). Waits while the database
    /// is suspended.
    pub fn read(&self) -> Result<ReadTxn<'_>> {
        self.inner.enter_snapshot();
        let snap = SnapGuard(&self.inner);
        Ok(ReadTxn {
            r: self.inner.storage.begin_read()?,
            dict: self.inner.dict,
            _snap: snap,
            _db: std::marker::PhantomData,
        })
    }

    /// An interactive write transaction in this process, holding the writer
    /// for at most the configured time limit.
    pub fn write(&self, durability: Durability) -> Result<WriteTxn<'_>> {
        let deadline = Instant::now() + self.inner.opts.write_timeout;
        let (mut st, ticket) = self.inner.acquire(self.inner.lock(), Some(deadline));
        if let Err(e) = self.inner.ensure_batch(&mut st) {
            self.inner.release(&mut st, ticket);
            return Err(e);
        }
        Ok(WriteTxn {
            db: self,
            ticket,
            deadline,
            durability,
            writes: Some(WriteSet::default()),
            blanks: Default::default(),
            interned: Vec::new(),
        })
    }

    /// Runs a one-shot write request: all operations or none, acknowledged once
    /// its durability level holds ([[adr:0026]]).
    pub fn submit(&self, req: WriteRequest, durability: Durability) -> Result<CommitTicket> {
        let (results, batch) = self.inner.request(durability, |st| {
            req.ops
                .iter()
                .map(|op| run_op(st, op))
                .collect::<Result<Vec<_>>>()
        })?;
        Ok(CommitTicket { results, batch })
    }

    /// Runs a SPARQL query on a fresh snapshot, through the plan cache
    /// (rules `plan-cache-key`, `plan-cache-invalidate`).
    pub fn sparql(&self, query: &str) -> Result<QueryResult> {
        self.sparql_with(query, &QueryOptions::default())
    }

    pub fn sparql_with(&self, query: &str, opts: &QueryOptions) -> Result<QueryResult> {
        let r = self.read()?;
        let mut ex = query_exec(&r, opts)?;
        let stats = crate::query::plan::Stats::load(&r.r)?;
        ex.stats = Some(stats.clone());
        let key = format!(
            "{}|{}|{:?}",
            opts.standard,
            opts.base_iri.as_deref().unwrap_or(""),
            query
        );
        let q = crate::sparql::parse_query(query, opts.base_iri.as_deref())?;
        let planned = match self.inner.plans.get(&key, &stats) {
            Some(p) => p,
            None => {
                let before = ex.transient_count();
                let p = crate::sparql::plan(&ex, &q, opts)?;
                if p.cacheable && ex.transient_count() == before {
                    let mut preds = Vec::new();
                    crate::query::plan::plan_preds(&p.node, &mut preds);
                    self.inner.plans.put(key, p.clone(), stats, preds);
                }
                p
            }
        };
        let res = crate::sparql::execute(&ex, &planned, &crate::sparql::describe_iris(&ex, &q)?);
        for (k, n) in ex.wanted.get().iter().enumerate() {
            self.inner.wanted[k].fetch_add(*n, std::sync::atomic::Ordering::Relaxed);
        }
        res
    }

    /// How often plans wanted each on-demand ordering (PSO, SOP, OPS) that
    /// was not ready.
    pub fn ordering_misses(&self) -> [u64; 3] {
        let w = &self.inner.wanted;
        [0, 1, 2].map(|k| w[k].load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Runs a query in SPARQL or Datalog and keeps the result as IDs with a
    /// snapshot to decode them (for the C FFI, rule `api-ffi-rows`).
    pub fn query_ids(
        &self,
        lang: Lang,
        text: &str,
    ) -> Result<(ReadTxn<'_>, crate::sparql::IdRows)> {
        let r = self.read()?;
        let rows = r.query_ids(lang, text)?;
        Ok((r, rows))
    }

    /// Durable batch commits since open (diagnostics).
    #[doc(hidden)]
    pub fn durable_commits(&self) -> u64 {
        self.inner.lock().durable_commits
    }

    /// The plan cache counters: (hits, misses, invalidations).
    pub fn plan_cache_stats(&self) -> (u64, u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let c = &self.inner.plans;
        (
            c.hits.load(Relaxed),
            c.misses.load(Relaxed),
            c.invalidations.load(Relaxed),
        )
    }

    /// Runs a SPARQL update request as one transaction.
    pub fn sparql_update(&self, update: &str, durability: Durability) -> Result<UpdateReport> {
        self.sparql_update_with(update, durability, &QueryOptions::default())
    }

    pub fn sparql_update_with(
        &self,
        update: &str,
        durability: Durability,
        opts: &QueryOptions,
    ) -> Result<UpdateReport> {
        let (rep, _) = self.inner.request(durability, |st| {
            crate::sparql::update::update(st, update, opts)
        })?;
        Ok(rep)
    }

    /// Runs a read-only Datalog program on a fresh snapshot; `@assert` is
    /// rejected ([[adr:0040]]).
    pub fn datalog(&self, program: &str) -> Result<QueryResult> {
        self.read()?.datalog(program)
    }

    /// Runs a Datalog program in a write request: `@assert` heads are written
    /// after evaluation, all or nothing.
    pub fn datalog_update(&self, program: &str, durability: Durability) -> Result<QueryResult> {
        let (res, _) = self
            .inner
            .request(durability, |st| datalog_in_store(st, program))?;
        Ok(res)
    }

    /// Administration: sweep and on-demand structures.
    pub fn admin(&self) -> Admin<'_> {
        Admin { db: self }
    }

    /// Size of the database file in bytes.
    pub fn file_size(&self) -> Result<u64> {
        self.inner.storage.file_size()
    }

    // @tg: implements:: [[openspec:mobile-bindings#iOS suspend and resume]]
    // @tg: implements:: [[openspec:mobile-bindings#Android background and kill safety]] {confidence: partial}
    /// Suspends the database for an app moving to the background
    /// ([[16-api#Mobile bindings]], rule `api-ios-suspend`), in this order:
    ///
    /// 1. new reads and writes wait until [`Database::resume`];
    /// 2. the open interactive write transaction is aborted (its next call
    ///    fails with [`Error::Aborted`]); one-shot requests already merged into
    ///    the batch stay in it;
    /// 3. the open batch is committed with a flush;
    /// 4. with `release_lock` (a file in an iOS app-group container), it waits
    ///    up to one second for open snapshots to end, then drops the engine
    ///    handle, which releases the OS file lock. A snapshot still open after
    ///    the wait gives [`Error::Busy`]; the database stays suspended with the
    ///    lock held.
    ///
    /// Without `release_lock` (the `ios-private-container` exception) the lock
    /// is kept and resume does not reopen the file. Calling it again while
    /// suspended returns `Ok` without effect.
    pub fn suspend(&self, release_lock: bool) -> Result<()> {
        let inner = &self.inner;
        {
            let mut g = inner.gate();
            if g.suspended {
                return Ok(());
            }
            g.suspended = true;
        }
        {
            let mut st = inner.lock();
            st.suspended = true;
            if let Some((t, Some(_))) = st.owner {
                // An interactive writer: its writes live in its own write
                // set, so revoking the token discards them.
                st.owner = None;
                st.revoked = Some(t);
            }
            inner.flush_durable(&mut st);
            inner.cv.notify_all();
            if st.failed_from.is_some() {
                return Err(Error::ReadOnly);
            }
        }
        if release_lock {
            let deadline = Instant::now() + SUSPEND_SNAPSHOT_WAIT;
            let mut g = inner.gate();
            while g.snapshots > 0 {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(Error::Busy(format!(
                        "{} read snapshot(s) still open; the file lock is kept",
                        g.snapshots
                    )));
                }
                g = inner
                    .gate_cv
                    .wait_timeout(g, left)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
            inner.storage.release();
        }
        Ok(())
    }

    /// Ends a suspension: reacquires the file lock when suspend released it,
    /// then lets waiting reads and writes run on fresh snapshots. Fails with
    /// [`Error::Locked`] (staying suspended) when another owner took the
    /// file meanwhile. A no-op when not suspended.
    pub fn resume(&self) -> Result<()> {
        let inner = &self.inner;
        {
            let mut g = inner.gate();
            if !g.suspended {
                return Ok(());
            }
            inner.storage.reacquire()?;
            g.suspended = false;
            inner.gate_cv.notify_all();
        }
        let mut st = inner.lock();
        st.suspended = false;
        inner.cv.notify_all();
        Ok(())
    }

    /// Whether the database is suspended.
    pub fn is_suspended(&self) -> bool {
        self.inner.gate().suspended
    }

    /// Commits the open batch with a flush now, so every acknowledged commit,
    /// `relaxed` ones included, survives a process kill (Android `ON_STOP`).
    /// An open interactive write transaction is not affected.
    pub fn flush(&self) -> Result<()> {
        let inner = &self.inner;
        let mut st = inner.lock();
        inner.flush_durable(&mut st);
        if st.failed_from.is_some() {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    /// Number of open read snapshots (diagnostics).
    pub fn open_snapshots(&self) -> usize {
        self.inner.gate().snapshots
    }

    /// How often resume reopened the file (diagnostics).
    #[doc(hidden)]
    pub fn storage_reopens(&self) -> u64 {
        self.inner.storage.reopens()
    }

    // @tg: implements:: [[openspec:mobile-bindings#iOS extension writes]] {confidence: partial}
    /// Applies `body` as one write request unless sequence number `seq` was
    /// already applied for `key`, and records `seq` under the metadata key
    /// `journal_seq:<key>` in the same transaction, so each sequence number
    /// is applied at most once even across a crash (iOS extension journal,
    /// [[16-api#Mobile bindings]]). Returns whether it applied the request.
    /// A failing request records nothing.
    pub fn submit_once(
        &self,
        key: &str,
        seq: u64,
        body: &RequestBody,
        durability: Durability,
    ) -> Result<bool> {
        let k = once_key(key);
        let (applied, _) = self.inner.request(durability, |st| {
            if seq <= tb::meta_u64(st.ro(), &k)?.unwrap_or(0) {
                return Ok(false);
            }
            match body {
                RequestBody::Ops(req) => {
                    for op in &req.ops {
                        run_op(st, op)?;
                    }
                }
                RequestBody::SparqlUpdate(text) => {
                    crate::sparql::update::update(st, text, &QueryOptions::default())?;
                }
                RequestBody::Datalog(program) => {
                    datalog_in_store(st, program)?;
                }
            }
            tb::meta_put_u64(st.kv, &k, seq)?;
            Ok(true)
        })?;
        Ok(applied)
    }

    /// The highest sequence number [`Database::submit_once`] applied for
    /// `key`, or 0.
    pub fn once_seq(&self, key: &str) -> Result<u64> {
        let r = self.read()?;
        Ok(tb::meta_u64(&r.r, &once_key(key))?.unwrap_or(0))
    }

    /// Writes a consistent copy of the database file to `to`: commits the
    /// open batch with a flush and copies the file while holding the writer,
    /// so writes wait for the copy.
    pub fn export_snapshot(&self, to: impl AsRef<Path>) -> Result<()> {
        let inner = &self.inner;
        let (mut st, ticket) = inner.acquire(inner.lock(), None);
        inner.flush_durable(&mut st);
        let res = if st.failed_from.is_some() {
            Err(Error::ReadOnly)
        } else {
            std::fs::copy(inner.storage.path(), to.as_ref())
                .map(|_| ())
                .map_err(Error::from)
        };
        inner.release(&mut st, ticket);
        res
    }

    /// Commits any open batch and closes the file.
    pub fn close(mut self) -> Result<()> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<()> {
        {
            let mut st = self.inner.lock();
            st.shutdown = true;
            if st.batch.is_some() {
                // A batch closes when the database closes (rule `tx-group-close`).
                if let Some(b) = st.batch.as_mut() {
                    b.durable = true;
                }
                self.inner.close_batch(&mut st);
            }
            self.inner.cv.notify_all();
        }
        if let Some(h) = self.flusher.take() {
            let _ = h.join();
        }
        let st = self.inner.lock();
        if st.failed_from.is_some() {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

// ---- reads ----------------------------------------------------------------------

/// Read operations shared by read and write transactions ([[16-api#Core API]]).
pub trait Reader {
    #[doc(hidden)]
    fn kv(&self) -> &dyn KvRead;
    #[doc(hidden)]
    fn dict(&self) -> Dict;

    /// The triple of a statement, if it exists (one `stmt` read).
    fn stmt(&self, id: Id) -> Result<Option<Triple>> {
        Ok(store::stmt_get(self.kv(), id)?.map(|(t, _)| t))
    }
    /// Whether a statement exists only as a reifier target ([[adr:0032]]).
    fn is_unasserted(&self, id: Id) -> Result<bool> {
        Ok(store::stmt_get(self.kv(), id)?.is_some_and(|(_, f)| f & store::flag::UNASSERTED != 0))
    }
    /// All inline properties of a node (one record read).
    fn props(&self, node: Id) -> Result<Vec<Match>> {
        store::props(self.kv(), node)
    }
    /// Statements with subject `s` (SPO prefix) and its inline properties.
    fn out(&self, s: Id, p: Option<Id>) -> Result<Vec<Match>> {
        store::find(self.kv(), Some(s), p, None)
    }
    /// Statements with object `o` (POS or OSP prefix).
    fn inc(&self, o: Id, p: Option<Id>) -> Result<Vec<Match>> {
        store::find(self.kv(), None, p, Some(o))
    }
    /// Every statement matching a triple pattern.
    fn find(&self, s: Option<Id>, p: Option<Id>, o: Option<Id>) -> Result<Vec<Match>> {
        store::find(self.kv(), s, p, o)
    }
    /// Inline values of `p` in an ID range (value order within a subtype);
    /// the flag says whether the value index served it.
    fn find_range(&self, p: Id, lo: Bound<Id>, hi: Bound<Id>) -> Result<(Vec<Match>, bool)> {
        store::find_range(self.kv(), p, lo, hi)
    }
    /// Statements of `p` whose literal value equals `value`, by content.
    fn find_value(&self, p: Id, value: &crate::term::Literal) -> Result<Vec<Match>> {
        store::find_value(self.kv(), &self.dict(), p, value)
    }
    /// The ID of a term, without interning it.
    fn lookup(&self, term: &Term) -> Result<Option<Id>> {
        self.dict().lookup(self.kv(), term)
    }
    /// Decodes an ID to its term.
    fn decode(&self, id: Id) -> Result<Term> {
        self.dict().decode(self.kv(), id)
    }
    /// Scans an on-demand or base ordering by prefix, once it is usable.
    fn scan_ordering(&self, perm: Perm, prefix: &[Id]) -> Result<Option<Vec<Match>>> {
        if !Perm::BASE.contains(&perm) && store::ordering_state(self.kv(), perm)? != 2 {
            return Ok(None);
        }
        let mut out = Vec::new();
        for multi in [false, true] {
            for e in crate::storage::prefix_iter(self.kv(), perm.table(multi), &tb::key(prefix)) {
                let (k, v) = e?;
                let (s, p, o) = perm.spo(tb::unkey3(&k));
                let id = if multi {
                    Id::from_be(&k[24..])
                } else {
                    Id::from_le(&v)
                };
                out.push(Match {
                    s,
                    p,
                    o,
                    id: Some(id),
                });
            }
        }
        Ok(Some(out))
    }
    /// The graph named by an IRI, without creating it.
    fn graph(&self, name: &Term) -> Result<Option<Id>> {
        crate::graph::graph_named(self.kv(), &self.dict(), name)
    }
    /// The home graph of an item; the default graph for other nodes.
    fn home_graph(&self, item: Id) -> Result<Id> {
        crate::graph::home(self.kv(), item)
    }
    /// The graphs a statement or hyperedge is a direct member of (`GRAPH ?g`).
    fn graphs_of(&self, item: Id) -> Result<Vec<Id>> {
        crate::graph::direct_graphs(self.kv(), item)
    }
    /// The graphs a pattern match is a direct member of.
    fn graphs_of_match(&self, m: &Match) -> Result<Vec<Id>> {
        crate::graph::match_graphs(self.kv(), m)
    }
    /// The promoted statement of membership `(item, :in, graph)`, if any.
    fn membership(&self, item: Id, graph: Id) -> Result<Option<Id>> {
        Ok(crate::graph::gmem_get(self.kv(), graph, item)?
            .map(|(_, p)| p)
            .filter(|p| !p.is_null()))
    }
    fn parents(&self, graph: Id) -> Result<Vec<Id>> {
        crate::graph::parents(self.kv(), graph)
    }
    fn children(&self, graph: Id) -> Result<Vec<Id>> {
        crate::graph::children(self.kv(), graph)
    }
    /// Every graph `graph` is nested under (`in_star`).
    fn ancestors(&self, graph: Id) -> Result<Vec<Id>> {
        crate::graph::ancestors(self.kv(), graph)
    }
    /// Every graph nested under `graph`.
    fn descendants(&self, graph: Id) -> Result<Vec<Id>> {
        crate::graph::descendants(self.kv(), graph)
    }
    /// The graphs a scope covers (`GRAPH` transitive, `GRAPH!` direct).
    fn scope(&self, graph: Id, direct: bool) -> Result<Vec<Id>> {
        crate::graph::scope_graphs(self.kv(), graph, direct)
    }
    /// The asserted statements in a scope, each once, in ID order.
    fn scope_statements(&self, graph: Id, direct: bool) -> Result<Vec<Id>> {
        crate::graph::scope_statements(self.kv(), graph, direct)
    }
    /// Statements and virtual triples matching a pattern inside a scope.
    fn find_in(
        &self,
        graph: Id,
        direct: bool,
        s: Option<Id>,
        p: Option<Id>,
        o: Option<Id>,
    ) -> Result<Vec<Match>> {
        crate::graph::find_in(self.kv(), graph, direct, s, p, o)
    }
    /// One hyperedge (one `hedge` read).
    fn hedge(&self, e: Id) -> Result<Option<Hyperedge>> {
        hedge::hedge(self.kv(), e)
    }
    fn hedge_type(&self, ty: Id) -> Result<Option<HedgeType>> {
        hedge::hedge_type(self.kv(), ty)
    }
    /// Every hyperedge of a type, or only those in a scope `(graph, direct)`.
    fn hedges_of_type(&self, ty: Id, scope: Option<(Id, bool)>) -> Result<Vec<Id>> {
        match scope {
            None => hedge::hedges_of_type(self.kv(), ty),
            Some((g, direct)) => hedge::hedges_of_type_in(self.kv(), ty, g, direct),
        }
    }
    /// The hyperedges node `n` fills, as `(role, type, hyperedge)`.
    fn incidence(&self, n: Id, role: Option<Id>, ty: Option<Id>) -> Result<Vec<(Id, Id, Id)>> {
        hedge::incidence(self.kv(), n, role, ty)
    }
    /// The promoted statement of a role edge, if any.
    fn role_id(&self, e: Id, role: Id, node: Id) -> Result<Option<Id>> {
        hedge::role_id(self.kv(), e, role, node)
    }
    /// Runs a SPARQL query on this view (no plan cache).
    fn sparql(&self, query: &str) -> Result<QueryResult> {
        self.sparql_with(query, &QueryOptions::default())
    }
    fn sparql_with(&self, query: &str, opts: &QueryOptions) -> Result<QueryResult> {
        let mut ex = crate::query::exec::Exec::new(self.kv(), self.dict());
        apply_budget(&mut ex, opts);
        ex.stats = Some(crate::query::plan::Stats::load(self.kv())?);
        crate::sparql::query(&ex, query, opts)
    }
    /// Runs a read-only Datalog program on this view.
    fn datalog(&self, program: &str) -> Result<QueryResult> {
        let ex = crate::query::exec::Exec::new(self.kv(), self.dict());
        Ok(crate::datalog::run_program(&ex, program, false)?.0)
    }
    /// The commit counter this view sees.
    fn commit_counter(&self) -> Result<u64> {
        Ok(tb::meta_u64(self.kv(), mk::COMMIT)?.unwrap_or(0))
    }
    /// Checks the cross-table invariants; `Some` names the first violation.
    fn check_invariants(&self) -> Result<Option<String>> {
        store::check_invariants(self.kv())
    }
}

/// A query language ([[16-api#Rust API]]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Sparql,
    Datalog,
}

impl ReadTxn<'_> {
    /// Runs a query in SPARQL or Datalog on this snapshot and keeps the result
    /// as IDs, decoded later with [`ReadTxn::decode_row_id`] (rule
    /// `api-rows-late`; used by the bindings).
    pub fn query_ids(&self, lang: Lang, text: &str) -> Result<crate::sparql::IdRows> {
        let r = self;
        let mut ex = crate::query::exec::Exec::new(&r.r, r.dict);
        ex.stats = Some(crate::query::plan::Stats::load(&r.r)?);
        Ok(match lang {
            Lang::Sparql => {
                let q = crate::sparql::parse_query(text, None)?;
                let p = crate::sparql::plan(&ex, &q, &QueryOptions::default())?;
                crate::sparql::execute_ids(&ex, &p, &crate::sparql::describe_iris(&ex, &q)?)?
            }
            Lang::Datalog => {
                let (res, _) = crate::datalog::run_program(&ex, text, false)?;
                let QueryResult::Solutions { vars, rows } = res else {
                    unreachable!()
                };
                let mut out = crate::sparql::IdRows {
                    vars,
                    ..Default::default()
                };
                for row in rows {
                    let mut ids = Vec::with_capacity(row.len());
                    for t in row {
                        ids.push(match t {
                            Some(t) => ex.encode(&t)?,
                            None => Id(0),
                        });
                    }
                    out.rows.push(ids);
                }
                out.transient = ex.transient_terms();
                out
            }
        })
    }

    /// Decodes an ID of a [`crate::sparql::IdRows`] result.
    pub fn decode_row_id(&self, rows: &crate::sparql::IdRows, id: Id) -> Result<Term> {
        rows.decode(&self.r, &self.dict, id)
    }
}

/// A read snapshot; any number may be open at once. It borrows the database,
/// so the file cannot be closed while a snapshot is alive.
pub struct ReadTxn<'db> {
    // Field order matters: the engine snapshot ends before the gate count
    // drops, so suspend never releases the file under a live snapshot.
    r: RedbRead,
    dict: Dict,
    _snap: SnapGuard<'db>,
    _db: std::marker::PhantomData<&'db Database>,
}

/// Uncounts a snapshot when its `ReadTxn` drops.
struct SnapGuard<'db>(&'db Inner);

impl Drop for SnapGuard<'_> {
    fn drop(&mut self) {
        self.0.leave_snapshot();
    }
}

impl std::fmt::Debug for ReadTxn<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadTxn")
    }
}

impl Reader for ReadTxn<'_> {
    fn kv(&self) -> &dyn KvRead {
        &self.r
    }
    fn dict(&self) -> Dict {
        self.dict
    }
}

// ---- interactive writes -----------------------------------------------------------

/// An interactive write transaction: sees its own writes, commits once.
pub struct WriteTxn<'db> {
    db: &'db Database,
    ticket: u64,
    deadline: Instant,
    durability: Durability,
    writes: Option<WriteSet>,
    blanks: std::collections::HashMap<String, Id>,
    interned: Vec<Id>,
}

impl std::fmt::Debug for WriteTxn<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteTxn")
            .field("ticket", &self.ticket)
            .finish_non_exhaustive()
    }
}

impl WriteTxn<'_> {
    /// Runs one call in a nested overlay so a failing call undoes only itself.
    /// Checks the token and the time limit first; a late transaction is
    /// aborted (rule `api-write-in-process`).
    fn call<R>(&mut self, f: impl FnOnce(&mut Store<'_>) -> Result<R>) -> Result<R> {
        let inner = Arc::clone(&self.db.inner);
        let Some(writes) = self.writes.take() else {
            return Err(Error::WriteTimeout);
        };
        let mut st = inner.lock();
        let mine = st.owner.is_some_and(|(t, _)| t == self.ticket);
        if !mine && st.revoked == Some(self.ticket) {
            return Err(suspend_aborted());
        }
        if !mine || Instant::now() >= self.deadline {
            if mine {
                inner.release(&mut st, self.ticket);
            }
            return Err(Error::WriteTimeout);
        }
        if let Err(e) = inner.ensure_batch(&mut st) {
            self.writes = Some(writes);
            return Err(e);
        }
        let base = &st.batch.as_ref().expect("batch").w;
        let mut outer = Overlay::with(base, writes);
        let r = {
            let mut inner_ov = Overlay::new(&outer);
            let mut store = Store::new(&mut inner_ov, inner.dict, inner.store_cfg());
            store.blanks = std::mem::take(&mut self.blanks);
            let r = f(&mut store);
            self.blanks = std::mem::take(&mut store.blanks);
            self.interned.append(&mut store.interned);
            r.map(|v| (v, inner_ov.into_writes()))
        };
        let r = match r {
            Ok((v, ws)) => ws.apply(&mut outer).map(|_| v),
            Err(e) => Err(e),
        };
        self.writes = Some(outer.into_writes());
        r
    }

    pub fn insert(
        &mut self,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Option<Id>> {
        let (s, p, o) = (s.into(), p.into(), o.into());
        self.call(|st| st.insert(&s, &p, &o))
    }
    pub fn set(
        &mut self,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Option<Id>> {
        let (s, p, o) = (s.into(), p.into(), o.into());
        self.call(|st| st.set(&s, &p, &o))
    }
    pub fn update(
        &mut self,
        id: Id,
        o: impl Into<TermRef>,
        expected: Option<TermRef>,
    ) -> Result<()> {
        let o = o.into();
        self.call(|st| st.update(id, &o, expected.as_ref()))
    }
    pub fn annotate(
        &mut self,
        id: Id,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Option<Id>> {
        let (p, o) = (p.into(), o.into());
        self.call(|st| st.annotate(id, &p, &o))
    }
    pub fn delete(&mut self, id: Id, mode: DeleteMode) -> Result<Vec<Id>> {
        self.call(|st| st.delete(id, mode))
    }
    pub fn remove(
        &mut self,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
        mode: DeleteMode,
    ) -> Result<bool> {
        let (s, p, o) = (s.into(), p.into(), o.into());
        self.call(|st| st.remove(&s, &p, &o, mode))
    }
    /// The statement ID of an existing triple, promoting an inline property.
    pub fn statement_id(
        &mut self,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Option<Id>> {
        let (s, p, o) = (s.into(), p.into(), o.into());
        self.call(|st| st.statement_id(&s, &p, &o))
    }
    pub fn insert_unasserted(
        &mut self,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Id> {
        let (s, p, o) = (s.into(), p.into(), o.into());
        self.call(|st| st.insert_unasserted(&s, &p, &o))
    }
    /// Inserts into a graph (an IRI or graph ID; a new name creates it).
    pub fn insert_in(
        &mut self,
        graph: impl Into<TermRef>,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<Option<Id>> {
        let (g, s, p, o) = (graph.into(), s.into(), p.into(), o.into());
        self.call(|st| st.insert_in(Some(&g), &s, &p, &o))
    }
    /// Removes a triple's membership in one graph; the statement goes with
    /// its last membership.
    pub fn remove_in(
        &mut self,
        graph: impl Into<TermRef>,
        s: impl Into<TermRef>,
        p: impl Into<TermRef>,
        o: impl Into<TermRef>,
    ) -> Result<bool> {
        let (g, s, p, o) = (graph.into(), s.into(), p.into(), o.into());
        self.call(|st| st.remove_in(Some(&g), &s, &p, &o, DeleteMode::Cascade))
    }
    /// Creates a named graph, or returns the existing one.
    pub fn create_graph(&mut self, name: impl Into<TermRef>) -> Result<Id> {
        let name = name.into();
        self.call(|st| st.graph(&name, true))
    }
    pub fn add_member(&mut self, item: Id, graph: impl Into<TermRef>) -> Result<bool> {
        let g = graph.into();
        self.call(|st| {
            let g = st.graph(&g, true)?;
            st.add_member(item, g)
        })
    }
    pub fn remove_member(&mut self, item: Id, graph: impl Into<TermRef>) -> Result<Vec<Id>> {
        let g = graph.into();
        self.call(|st| {
            let g = st.graph(&g, false)?;
            st.remove_member(item, g)
        })
    }
    pub fn move_member(
        &mut self,
        item: Id,
        from: impl Into<TermRef>,
        to: impl Into<TermRef>,
    ) -> Result<Vec<Id>> {
        let (from, to) = (from.into(), to.into());
        self.call(|st| {
            let (f, t) = (st.graph(&from, false)?, st.graph(&to, true)?);
            st.move_member(item, f, t)
        })
    }
    /// The statement ID of the membership `(item, :in, graph)`, promoting it.
    pub fn membership_id(&mut self, item: Id, graph: impl Into<TermRef>) -> Result<Id> {
        let g = graph.into();
        self.call(|st| {
            let g = st.graph(&g, false)?;
            st.membership_id(item, g)
        })
    }
    /// Nests `child` under `parent` (graphs named by IRI are created).
    pub fn nest(&mut self, child: impl Into<TermRef>, parent: impl Into<TermRef>) -> Result<bool> {
        let (c, p) = (child.into(), parent.into());
        self.call(|st| {
            let (c, p) = (st.graph(&c, true)?, st.graph(&p, true)?);
            st.nest(c, p)
        })
    }
    pub fn unnest(
        &mut self,
        child: impl Into<TermRef>,
        parent: impl Into<TermRef>,
    ) -> Result<bool> {
        let (c, p) = (child.into(), parent.into());
        self.call(|st| {
            let (c, p) = (st.graph(&c, false)?, st.graph(&p, false)?);
            st.unnest(c, p)
        })
    }
    /// `DROP [RECURSIVE] GRAPH`; returns the deleted items.
    pub fn drop_graph(&mut self, graph: impl Into<TermRef>, recursive: bool) -> Result<Vec<Id>> {
        let g = graph.into();
        self.call(|st| {
            let g = st.graph(&g, false)?;
            st.drop_graph(g, recursive)
        })
    }
    pub fn declare_hedge_type(
        &mut self,
        ty: impl Into<TermRef>,
        decl: HedgeTypeDecl,
    ) -> Result<Id> {
        let ty = ty.into();
        self.call(|st| st.declare_hedge_type(&ty, &decl))
    }
    /// Creates a hyperedge in `graph` (default graph when `None`).
    pub fn create_hedge(
        &mut self,
        ty: impl Into<TermRef>,
        graph: Option<TermRef>,
        roles: Vec<(TermRef, TermRef)>,
        attrs: Vec<(TermRef, TermRef)>,
    ) -> Result<Id> {
        let ty = ty.into();
        self.call(|st| st.create_hedge(&ty, graph.as_ref(), &roles, &attrs))
    }
    pub fn update_role(
        &mut self,
        hedge: Id,
        role: impl Into<TermRef>,
        filler: impl Into<TermRef>,
    ) -> Result<()> {
        let (r, f) = (role.into(), filler.into());
        self.call(|st| st.update_role(hedge, &r, &f))
    }
    /// The statement ID of role edge `(hedge, role, node)`, promoting it.
    pub fn role_id(
        &mut self,
        hedge: Id,
        role: impl Into<TermRef>,
        node: impl Into<TermRef>,
    ) -> Result<Id> {
        let (r, n) = (role.into(), node.into());
        self.call(|st| {
            let (r, n) = (st.intern(&r)?, st.intern(&n)?);
            st.role_statement(hedge, r, n)
        })
    }
    /// Interns a term and returns its ID.
    pub fn intern(&mut self, t: impl Into<TermRef>) -> Result<Id> {
        let t = t.into();
        self.call(|st| st.intern(&t))
    }
    pub fn declare_predicate(&mut self, p: impl Into<TermRef>, decl: PredicateDecl) -> Result<()> {
        let p = p.into();
        self.call(|st| st.declare_predicate(&p, decl))
    }
    pub fn run(&mut self, op: &CoreOp) -> Result<OpResult> {
        self.call(|st| run_op(st, op))
    }

    /// Runs a SPARQL update inside this transaction.
    pub fn sparql_update(&mut self, update: &str) -> Result<UpdateReport> {
        self.call(|st| crate::sparql::update::update(st, update, &QueryOptions::default()))
    }
    /// Runs a Datalog program inside this transaction; `@assert` heads write.
    pub fn datalog(&mut self, program: &str) -> Result<QueryResult> {
        self.call(|st| datalog_in_store(st, program))
    }
    /// Runs a SPARQL query on this transaction's own writes.
    pub fn sparql(&mut self, query: &str) -> Result<QueryResult> {
        self.read(|r| r.sparql(query))
    }

    /// Reads see this transaction's own writes (rule `tx-single-writer`).
    pub fn read<R>(&mut self, f: impl FnOnce(&dyn ReaderView) -> Result<R>) -> Result<R> {
        self.call(|st| {
            let view = StoreView {
                kv: st.kv.as_read(),
                dict: st.dict,
            };
            f(&view)
        })
    }

    /// Commits; returns once the durability level holds.
    pub fn commit(mut self) -> Result<CommitTicket> {
        let inner = Arc::clone(&self.db.inner);
        let writes = self.writes.take();
        let mut st = inner.lock();
        let mine = st.owner.is_some_and(|(t, _)| t == self.ticket);
        if !mine && st.revoked == Some(self.ticket) {
            return Err(suspend_aborted());
        }
        let Some(writes) = writes.filter(|_| mine && Instant::now() < self.deadline) else {
            if mine {
                inner.release(&mut st, self.ticket);
            }
            return Err(Error::WriteTimeout);
        };
        inner.note_interned(&self.interned);
        let res = inner.merge(&mut st, writes, self.durability);
        inner.release(&mut st, self.ticket);
        let seq = res?;
        if self.durability.durable() {
            inner.wait_durable(st, seq)?;
        }
        Ok(CommitTicket {
            results: Vec::new(),
            batch: seq,
        })
    }

    /// Discards every write of this transaction.
    pub fn abort(mut self) {
        self.release_token();
    }

    fn release_token(&mut self) {
        self.writes = None;
        let inner = &self.db.inner;
        let mut st = inner.lock();
        inner.release(&mut st, self.ticket);
    }
}

impl Drop for WriteTxn<'_> {
    fn drop(&mut self) {
        if self.writes.is_some() {
            self.release_token();
        }
    }
}

/// Read access inside a write transaction.
pub trait ReaderView: Reader {}

struct StoreView<'a> {
    kv: &'a dyn KvRead,
    dict: Dict,
}

impl Reader for StoreView<'_> {
    fn kv(&self) -> &dyn KvRead {
        self.kv
    }
    fn dict(&self) -> Dict {
        self.dict
    }
}

impl ReaderView for StoreView<'_> {}

// ---- administration --------------------------------------------------------------

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub marked: u64,
    pub examined: u64,
    pub deleted: u64,
    pub kept_young: u64,
    pub kept_rereferenced: u64,
}

/// Administrative operations.
#[derive(Debug)]
pub struct Admin<'db> {
    db: &'db Database,
}

const BUILD_BATCH: usize = 4096;
const SWEEP_BATCH: usize = 1024;

impl Admin<'_> {
    /// Builds an on-demand ordering online, in batches (rule `pl-ondemand-online`).
    pub fn build_ordering(&self, perm: Perm) -> Result<()> {
        let inner = &self.db.inner;
        inner.request(Durability::Ordered, |st| st.begin_ordering(perm))?;
        if Perm::BASE.contains(&perm) {
            return Ok(());
        }
        let mut cursor = None;
        loop {
            let (next, _) = inner.request(Durability::Ordered, |st| {
                st.build_ordering_batch(perm, cursor.clone(), BUILD_BATCH)
            })?;
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        inner.request(Durability::Ordered, |_| Ok(()))?;
        Ok(())
    }

    /// Builds every on-demand ordering that plans wanted at least `threshold`
    /// times while it was missing (rule `plan-ordering-build`, [[adr:0041]]),
    /// then drops cached plans so the next queries see it. Returns the
    /// orderings built.
    pub fn build_wanted_orderings(&self, threshold: u64) -> Result<Vec<Perm>> {
        let misses = self.db.ordering_misses();
        let mut built = Vec::new();
        for (k, perm) in Perm::ON_DEMAND.iter().enumerate() {
            let ready = store::ordering_state(&self.db.read()?.r, *perm)? == 2;
            if misses[k] >= threshold && !ready {
                self.build_ordering(*perm)?;
                built.push(*perm);
            }
        }
        if !built.is_empty() {
            self.db.inner.plans.clear();
            for k in 0..3 {
                self.db.inner.wanted[k].store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok(built)
    }

    /// Builds a value index on `p` online (rules `dm-value-index-auto`, `pl-sort-key`).
    pub fn build_value_index(&self, p: &Term) -> Result<()> {
        let inner = &self.db.inner;
        let (p, _) = inner.request(Durability::Ordered, |st| {
            let p = st.intern(&TermRef::Term(p.clone()))?;
            st.begin_value_index(p)?;
            Ok(p)
        })?;
        let mut cursor = None;
        loop {
            let (next, _) = inner.request(Durability::Ordered, |st| {
                st.build_value_index_batch(p, cursor, BUILD_BATCH)
            })?;
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        inner.request(Durability::Ordered, |_| Ok(()))?;
        Ok(())
    }

    /// Whether the dictionary grew 20% since the last completed sweep.
    pub fn sweep_due(&self) -> Result<bool> {
        let r = self.db.read()?;
        let size = tb::meta_u64(&r.r, mk::CTR_IRI)?.unwrap_or(1)
            + tb::meta_u64(&r.r, mk::CTR_LIT)?.unwrap_or(1);
        let last = tb::meta_u64(&r.r, mk::SWEEP_LAST_SIZE)?.unwrap_or(0);
        Ok(size * 5 > last * 6)
    }

    /// Runs a full mark-and-sweep of the dictionary
    /// ([[03-dictionary#Mark and sweep]]). Terms interned less than `window`
    /// ago are kept (rule `dict-sweep-window`).
    pub fn sweep(&self, window: Duration) -> Result<SweepReport> {
        self.sweep_hooked(window, || ())
    }

    /// [`Admin::sweep`] with a hook that runs after the mark pass and before
    /// the first sweep batch; tests use it to write in between.
    #[doc(hidden)]
    pub fn sweep_hooked(&self, window: Duration, after_mark: impl FnOnce()) -> Result<SweepReport> {
        let inner = &self.db.inner;
        // Close the open batch under the writer token, then start the kept set
        // and take the mark snapshot, so every reference is either in the
        // snapshot or recorded in the kept set (rule `dict-sweep-race`).
        let snap = {
            let (mut st, ticket) = inner.acquire(inner.lock(), None);
            inner.close_batch(&mut st);
            *inner.kept.lock().unwrap_or_else(|p| p.into_inner()) = Some(HashSet::new());
            // Counted like any snapshot, so suspend does not release the file
            // lock under a running sweep.
            inner.gate().snapshots += 1;
            let snap = inner.storage.begin_read();
            inner.release(&mut st, ticket);
            match snap {
                Ok(s) => s,
                Err(e) => {
                    inner.leave_snapshot();
                    *inner.kept.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    return Err(e);
                }
            }
        };
        let result = self.sweep_from(snap, window, after_mark);
        inner.leave_snapshot();
        *inner.kept.lock().unwrap_or_else(|p| p.into_inner()) = None;
        result
    }

    fn sweep_from(
        &self,
        snap: RedbRead,
        window: Duration,
        after_mark: impl FnOnce(),
    ) -> Result<SweepReport> {
        let inner = &self.db.inner;
        let mut rep = SweepReport::default();
        let mut live = roaring::RoaringTreemap::new();
        let mut mark = |id: Id| {
            if matches!(id.kind(), Kind::Iri | Kind::Blank | Kind::DictLiteral)
                && !id.is_owned_literal()
            {
                live.insert(id.raw());
            }
        };
        // Mark: every structure of rule `dict-sweep-liveness` that exists in v0.
        for t in [&tb::SPO, &tb::SPO_M, &tb::UNASSERTED, &tb::PROPS_EXT] {
            for e in RangeIter::new(&snap, t, Bound::Unbounded, Bound::Unbounded, false) {
                let (k, _) = e?;
                for c in k.chunks_exact(8).take(3) {
                    mark(Id::from_be(c));
                }
            }
        }
        for e in RangeIter::new(&snap, &tb::PROPS, Bound::Unbounded, Bound::Unbounded, false) {
            let (k, _) = e?;
            let node = Id::from_be(&k);
            mark(node);
            for (p, v) in store::load_props(&snap, node)?.entries {
                mark(p);
                mark(v);
            }
        }
        for e in RangeIter::new(
            &snap,
            &tb::PROPIDX,
            Bound::Unbounded,
            Bound::Unbounded,
            false,
        ) {
            let (k, _) = e?;
            mark(Id::from_be(&k));
            mark(Id::from_be(&k[24..]));
        }
        for e in crate::storage::prefix_iter(&snap, &tb::META, b"pred:") {
            let (k, _) = e?;
            if let Ok(n) = u64::from_str_radix(std::str::from_utf8(&k[5..]).unwrap_or(""), 16) {
                mark(Id(n));
            }
        }
        // Graph names, hyperedge records and incidence, type declarations and
        // the system IRIs.
        for e in RangeIter::new(&snap, &tb::GNAME, Bound::Unbounded, Bound::Unbounded, false) {
            mark(Id::from_be(&e?.0));
        }
        for e in RangeIter::new(
            &snap,
            &tb::INCIDENCE,
            Bound::Unbounded,
            Bound::Unbounded,
            false,
        ) {
            let (k, _) = e?;
            for c in k.chunks_exact(8).take(3) {
                mark(Id::from_be(c));
            }
        }
        for (ty, decl) in hedge::hedge_types(&snap)? {
            mark(ty);
            for r in decl.roles {
                mark(r.role);
            }
            for a in decl.attrs {
                mark(a);
            }
        }
        for e in RangeIter::new(&snap, &tb::ALIAS, Bound::Unbounded, Bound::Unbounded, false) {
            mark(Id::from_be(&e?.0));
        }
        for k in [mk::SYS_IN, mk::SYS_TYPE] {
            if let Some(v) = tb::meta_u64(&snap, k)? {
                mark(Id(v));
            }
        }
        rep.marked = live.len();
        after_mark();
        // Safety window boundary (rule `dict-sweep-window`).
        let commit = tb::meta_u64(&snap, mk::COMMIT)?.unwrap_or(0);
        let boundary = if window.is_zero() {
            commit + 1
        } else {
            let cutoff = unix_micros().saturating_sub(window.as_micros() as u64);
            let mut b = 0;
            for e in RangeIter::new(
                &snap,
                &tb::TXCLOCK,
                Bound::Unbounded,
                Bound::Unbounded,
                true,
            ) {
                let (k, v) = e?;
                if u64::from_le_bytes(v[..8].try_into().expect("8")) <= cutoff {
                    b = u64::from_be_bytes(k[..8].try_into().expect("8"));
                    break;
                }
            }
            b
        };
        // Sweep in batches; each batch is its own write request.
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let lo = cursor.clone().map_or(Bound::Unbounded, Bound::Excluded);
            let page = {
                let lo_ref = match &lo {
                    Bound::Excluded(k) => Bound::Excluded(k.as_slice()),
                    _ => Bound::Unbounded,
                };
                snap.scan(&tb::DICT_REV, lo_ref, Bound::Unbounded, false, SWEEP_BATCH)?
            };
            if page.is_empty() {
                break;
            }
            cursor = Some(page.last().expect("page").0.clone());
            let kept = inner
                .kept
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .unwrap_or_default();
            let mut doomed = Vec::new();
            for (k, v) in &page {
                let id = Id::from_be(k);
                rep.examined += 1;
                if id.is_owned_literal() || live.contains(id.raw()) {
                    continue;
                }
                let rec = Record::decode(v)?;
                if rec.created_tx >= boundary {
                    rep.kept_young += 1;
                } else if kept.contains(&id) {
                    rep.kept_rereferenced += 1;
                } else {
                    doomed.push((id, rec));
                }
            }
            let cur = cursor.clone().expect("cursor");
            let (n, _) = inner.request(Durability::Ordered, |st| {
                let kept_now = inner
                    .kept
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
                    .unwrap_or_default();
                let mut n = 0;
                for (id, rec) in &doomed {
                    if kept_now.contains(id) {
                        continue;
                    }
                    delete_term(st.kv, *id, rec)?;
                    n += 1;
                }
                tb::meta_put(st.kv, mk::SWEEP_CURSOR, &cur)?;
                Ok(n)
            })?;
            rep.deleted += n;
        }
        let size_r = self.db.read()?;
        let size = tb::meta_u64(&size_r.r, mk::CTR_IRI)?.unwrap_or(1)
            + tb::meta_u64(&size_r.r, mk::CTR_LIT)?.unwrap_or(1);
        inner.request(Durability::Ordered, |st| {
            st.kv.del(&tb::META, mk::SWEEP_CURSOR.as_bytes())?;
            tb::meta_put_u64(st.kv, mk::SWEEP_LAST_SIZE, size)
        })?;
        Ok(rep)
    }
}

/// Deletes an interned term's `dict_rev` and `dict_fwd` entries.
fn delete_term(kv: &mut dyn KvWrite, id: Id, rec: &Record) -> Result<()> {
    let full = rec.full_key();
    if full.len() <= crate::dict::HASH_KEY_LIMIT {
        kv.del(&tb::DICT_FWD, &full)?;
    } else {
        let h = xxhash_rust::xxh3::xxh3_128(&full).to_be_bytes();
        let mut base = vec![0xFF];
        base.extend_from_slice(&h);
        for d in std::iter::once(None).chain((0..=255u8).map(Some)) {
            let mut k = base.clone();
            if let Some(d) = d {
                k.push(d);
            }
            match kv.get(&tb::DICT_FWD, &k)? {
                None => break,
                Some(v) if Id::from_le(&v) == id => {
                    kv.del(&tb::DICT_FWD, &k)?;
                    break;
                }
                Some(_) => {}
            }
        }
    }
    kv.del(&tb::DICT_REV, &id.be())?;
    let _ = id::K;
    Ok(())
}

fn apply_budget(ex: &mut crate::query::exec::Exec<'_>, opts: &QueryOptions) {
    ex.standard = opts.standard;
    if let Some(n) = opts.max_rows {
        ex.budget.max_rows = n;
    }
    if let Some(t) = opts.timeout {
        ex.budget.deadline = Some(Instant::now() + t);
    }
}

fn query_exec<'a>(r: &'a ReadTxn<'_>, opts: &QueryOptions) -> Result<crate::query::exec::Exec<'a>> {
    let mut ex = crate::query::exec::Exec::new(&r.r, r.dict);
    apply_budget(&mut ex, opts);
    Ok(ex)
}

/// Evaluates a Datalog program on the request's view, then writes what its
/// `@assert` rules derived (rule `ir-write-after-read`).
fn datalog_in_store(st: &mut Store<'_>, program: &str) -> Result<QueryResult> {
    let (res, writes) = {
        let ex = crate::query::exec::Exec::new(st.ro(), st.dict);
        crate::datalog::run_program(&ex, program, true)?
    };
    for [s, p, o] in writes {
        st.insert_in(Some(&TermRef::Id(DEFAULT_GRAPH)), &s, &p, &o)?;
    }
    Ok(res)
}
