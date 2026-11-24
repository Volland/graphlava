//! Agent-memory mix, latency/throughput drivers, overwrite phase, cyclic queries,
//! online ordering build. All generic over `Engine`.

use crate::data::*;
use crate::engine::*;
use hdrhistogram::Histogram;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Ds {
    pub nodes: u64,
    pub next_stmt: AtomicU64,
    pub ords: Vec<Tab>,
}

impl Ds {
    pub fn from_meta(m: &DsMeta) -> Self {
        Ds { nodes: m.nodes, next_stmt: AtomicU64::new(m.next_stmt), ords: ords_from_names(&m.ords) }
    }
    pub fn rnode(&self, rng: &mut SmallRng) -> u64 {
        node_id(rng.gen_range(0..self.nodes))
    }
    pub fn rstmt(&self, rng: &mut SmallRng) -> u64 {
        stmt_id(rng.gen_range(0..self.next_stmt.load(Ordering::Relaxed)))
    }
}

// ---------------------------------------------------------------- disk counters

/// Bytes written to storage by this process (macOS proc_pid_rusage RUSAGE_INFO_V2,
/// ri_diskio_byteswritten) plus getrusage ru_oublock (block output operations).
pub fn disk_written() -> (u64, i64) {
    unsafe {
        let mut ri: libc::rusage_info_v2 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            &mut ri as *mut _ as *mut libc::rusage_info_t,
        );
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        (if rc == 0 { ri.ri_diskio_byteswritten } else { 0 }, ru.ru_oublock as i64)
    }
}

// ---------------------------------------------------------------- op classes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    StmtGet,
    PropsGet,
    Hop1,
    Hop2,
    PropOverwrite,
    ObjOverwrite,
    Insert,
    Delete,
}
pub const ALL_OPS: [Op; 8] = [
    Op::StmtGet,
    Op::PropsGet,
    Op::Hop1,
    Op::Hop2,
    Op::PropOverwrite,
    Op::ObjOverwrite,
    Op::Insert,
    Op::Delete,
];
impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::StmtGet => "stmt_get",
            Op::PropsGet => "props_get",
            Op::Hop1 => "hop1_scan",
            Op::Hop2 => "hop2_scan",
            Op::PropOverwrite => "prop_overwrite",
            Op::ObjOverwrite => "obj_overwrite",
            Op::Insert => "insert_1_5",
            Op::Delete => "delete",
        }
    }
    pub fn is_write(self) -> bool {
        matches!(self, Op::PropOverwrite | Op::ObjOverwrite | Op::Insert | Op::Delete)
    }
}

/// The mix (percent): 20 stmt get, 20 props get, 10 one-hop scan, 10 two-hop scan,
/// 15 property overwrite, 10 object overwrite, 10 insert of 1-5 statements, 5 delete.
pub const MIX: [(Op, u32); 8] = [
    (Op::StmtGet, 20),
    (Op::PropsGet, 20),
    (Op::Hop1, 10),
    (Op::Hop2, 10),
    (Op::PropOverwrite, 15),
    (Op::ObjOverwrite, 10),
    (Op::Insert, 10),
    (Op::Delete, 5),
];

pub fn pick(rng: &mut SmallRng, mix: &[(Op, u32)]) -> Op {
    let tot: u32 = mix.iter().map(|x| x.1).sum();
    let mut r = rng.gen_range(0..tot);
    for &(op, w) in mix {
        if r < w {
            return op;
        }
        r -= w;
    }
    unreachable!()
}

pub struct OpCtx {
    pub buf: Vec<u8>,
    pub kv_bytes: u64,
    pub rows: u64,
}
impl OpCtx {
    pub fn new() -> Self {
        OpCtx { buf: Vec::with_capacity(256), kv_bytes: 0, rows: 0 }
    }
}

fn scan_prefix<R: ReadOps + ?Sized>(r: &R, t: Tab, prefix: &[u8], mut f: impl FnMut(&[u8], &[u8]) -> bool) {
    let (lo, hi) = prefix_bounds(prefix, 24);
    r.scan(t, &lo, &hi, &mut f);
}

fn ins<W: WriteOps>(w: &mut W, c: &mut OpCtx, t: Tab, k: &[u8], v: &[u8]) {
    c.kv_bytes += (k.len() + v.len()) as u64;
    w.insert(t, k, v);
}
fn rem<W: WriteOps>(w: &mut W, c: &mut OpCtx, t: Tab, k: &[u8]) -> bool {
    c.kv_bytes += k.len() as u64;
    w.remove(t, k)
}

/// Find a live statement inside the write txn (up to 8 tries).
fn find_live<W: WriteOps>(w: &W, ds: &Ds, rng: &mut SmallRng, buf: &mut Vec<u8>) -> Option<(u64, u64, u64, u64)> {
    for _ in 0..8 {
        let id = ds.rstmt(rng);
        if w.get(Tab::Stmt, &id.to_be_bytes(), buf) {
            let (s, p, o) = stmt_decode(buf);
            return Some((id, s, p, o));
        }
    }
    None
}

pub fn run_op<E: Engine>(e: &E, ds: &Ds, op: Op, d: Dur, rng: &mut SmallRng, c: &mut OpCtx) {
    match op {
        Op::StmtGet => {
            let r = e.begin_read();
            let id = ds.rstmt(rng);
            if r.get(Tab::Stmt, &id.to_be_bytes(), &mut c.buf) {
                c.rows += 1;
            }
        }
        Op::PropsGet => {
            let r = e.begin_read();
            let n = ds.rnode(rng);
            if r.get(Tab::Props, &n.to_be_bytes(), &mut c.buf) {
                c.rows += 1;
            }
        }
        Op::Hop1 => {
            let r = e.begin_read();
            let n = ds.rnode(rng);
            let t = if rng.gen_bool(0.5) { Tab::Spo } else { Tab::Osp };
            let mut rows = 0;
            scan_prefix(&r, t, &n.to_be_bytes(), |_, _| {
                rows += 1;
                rows < 1000
            });
            c.rows += rows;
        }
        Op::Hop2 => {
            let r = e.begin_read();
            let n = ds.rnode(rng);
            let mut objs: Vec<u64> = Vec::with_capacity(16);
            scan_prefix(&r, Tab::Spo, &n.to_be_bytes(), |k, _| {
                objs.push(u64::from_be_bytes(k[16..24].try_into().unwrap()));
                objs.len() < 16
            });
            let mut rows = objs.len() as u64;
            for o in objs {
                let mut m = 0;
                scan_prefix(&r, Tab::Spo, &o.to_be_bytes(), |_, _| {
                    m += 1;
                    m < 64
                });
                rows += m;
            }
            c.rows += rows;
        }
        Op::PropOverwrite => {
            let mut w = e.begin_write();
            let n = ds.rnode(rng);
            let nb = n.to_be_bytes();
            if w.get(Tab::Props, &nb, &mut c.buf) {
                let mut rec = c.buf.clone();
                let cnt = u16::from_le_bytes([rec[2], rec[3]]) as usize;
                let i = rng.gen_range(0..cnt);
                let off = 4 + 16 * i + 8;
                rec[off..off + 8].copy_from_slice(&int_lit(rng.gen_range(0..1_000_000)).to_le_bytes());
                ins(&mut w, c, Tab::Props, &nb, &rec);
            }
            w.commit(d);
        }
        Op::ObjOverwrite => {
            let mut w = e.begin_write();
            let mut buf = std::mem::take(&mut c.buf);
            if let Some((id, s, p, o)) = find_live(&w, ds, rng, &mut buf) {
                let o2 = rand_obj(rng, ds.nodes);
                let idb = id.to_le_bytes();
                for t in &ds.ords {
                    rem(&mut w, c, *t, &t.key(s, p, o));
                }
                for t in &ds.ords {
                    ins(&mut w, c, *t, &t.key(s, p, o2), &idb);
                }
                ins(&mut w, c, Tab::Stmt, &id.to_be_bytes(), &stmt_val(s, p, o2, 0));
            }
            c.buf = buf;
            w.commit(d);
        }
        Op::Insert => {
            let mut w = e.begin_write();
            let m = rng.gen_range(1..=5);
            for _ in 0..m {
                let k = ds.next_stmt.fetch_add(1, Ordering::Relaxed);
                let id = stmt_id(k);
                let (s, p, o) = (ds.rnode(rng), rand_pred(rng), rand_obj(rng, ds.nodes));
                let idb = id.to_le_bytes();
                for t in &ds.ords {
                    ins(&mut w, c, *t, &t.key(s, p, o), &idb);
                }
                ins(&mut w, c, Tab::Stmt, &id.to_be_bytes(), &stmt_val(s, p, o, 0));
            }
            w.commit(d);
        }
        Op::Delete => {
            let mut w = e.begin_write();
            let mut buf = std::mem::take(&mut c.buf);
            if let Some((id, s, p, o)) = find_live(&w, ds, rng, &mut buf) {
                for t in &ds.ords {
                    rem(&mut w, c, *t, &t.key(s, p, o));
                }
                rem(&mut w, c, Tab::Stmt, &id.to_be_bytes());
            }
            c.buf = buf;
            w.commit(d);
        }
    }
}

// ---------------------------------------------------------------- histograms

pub fn new_hist() -> Histogram<u64> {
    Histogram::<u64>::new_with_bounds(1, 120_000_000_000, 3).unwrap()
}

pub fn hist_summary(h: &Histogram<u64>) -> Value {
    let us = |ns: u64| (ns as f64) / 1000.0;
    json!({
        "count": h.len(),
        "p50_us": us(h.value_at_quantile(0.50)),
        "p99_us": us(h.value_at_quantile(0.99)),
        "p999_us": us(h.value_at_quantile(0.999)),
        "max_us": us(h.max()),
        "mean_us": h.mean() / 1000.0,
    })
}

pub fn hist_raw(h: &Histogram<u64>) -> String {
    use base64::Engine as _;
    use hdrhistogram::serialization::Serializer as _;
    let mut v = Vec::new();
    hdrhistogram::serialization::V2DeflateSerializer::new().serialize(h, &mut v).unwrap();
    base64::engine::general_purpose::STANDARD.encode(v)
}

// ---------------------------------------------------------------- latency run

/// In NoFlush runs, after every CHECKPOINT-th write op an empty Flush commit is issued
/// outside the timed region (a background checkpointer). Without it redb never frees
/// COW pages (they are released only by durable commits) and the file grows without bound.
pub static CHECKPOINT: AtomicU64 = AtomicU64::new(100);

fn maybe_checkpoint<E: Engine>(e: &E, d: Dur, op: Op, nw: &mut u64) {
    if d == Dur::NoFlush && op.is_write() {
        *nw += 1;
        let k = CHECKPOINT.load(Ordering::Relaxed);
        if k > 0 && *nw % k == 0 {
            let w = e.begin_write();
            w.commit(Dur::Flush);
        }
    }
}

/// Closed-loop, single thread: each op is timed from begin_txn to commit/return.
pub fn latency_run<E: Engine>(e: &E, ds: &Ds, mix: &[(Op, u32)], d: Dur, warmup: u64, n: u64, seed: u64) -> Value {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut c = OpCtx::new();
    let mut nw = 0u64;
    for _ in 0..warmup {
        let op = pick(&mut rng, mix);
        run_op(e, ds, op, d, &mut rng, &mut c);
        maybe_checkpoint(e, d, op, &mut nw);
    }
    let mut hs: Vec<Histogram<u64>> = (0..8).map(|_| new_hist()).collect();
    let mut all = new_hist();
    let mut wr = new_hist();
    let (dw0, ob0) = disk_written();
    c.kv_bytes = 0;
    let mut nwrites = 0u64;
    let t0 = Instant::now();
    for _ in 0..n {
        let op = pick(&mut rng, mix);
        let s = Instant::now();
        run_op(e, ds, op, d, &mut rng, &mut c);
        let ns = s.elapsed().as_nanos() as u64;
        let i = ALL_OPS.iter().position(|x| *x == op).unwrap();
        hs[i].record(ns.max(1)).unwrap();
        all.record(ns.max(1)).unwrap();
        if op.is_write() {
            wr.record(ns.max(1)).unwrap();
            nwrites += 1;
        }
        maybe_checkpoint(e, d, op, &mut nw);
    }
    let wall = t0.elapsed().as_secs_f64();
    // make NoFlush writes reach the device so the counter sees them
    if d == Dur::NoFlush {
        let w = e.begin_write();
        w.commit(Dur::Flush);
    }
    let (dw1, ob1) = disk_written();
    let mut per = serde_json::Map::new();
    let mut raw = serde_json::Map::new();
    for (i, op) in ALL_OPS.iter().enumerate() {
        if hs[i].len() > 0 {
            per.insert(op.name().into(), hist_summary(&hs[i]));
            raw.insert(op.name().into(), json!(hist_raw(&hs[i])));
        }
    }
    raw.insert("all".into(), json!(hist_raw(&all)));
    json!({
        "durability": format!("{:?}", d), "checkpoint_every_writes": CHECKPOINT.load(Ordering::Relaxed), "ops": n, "warmup": warmup, "seed": seed, "wall_s": wall,
        "ops_per_s": n as f64 / wall,
        "all": hist_summary(&all), "writes": hist_summary(&wr), "per_class": per,
        "disk_bytes_written": dw1 - dw0, "ru_oublock": ob1 - ob0, "write_ops": nwrites,
        "kv_bytes_changed": c.kv_bytes,
        "disk_bytes_per_write_op": if nwrites > 0 { (dw1 - dw0) as f64 / nwrites as f64 } else { 0.0 },
        "write_amp_vs_kv_bytes": if c.kv_bytes > 0 { (dw1 - dw0) as f64 / c.kv_bytes as f64 } else { 0.0 },
        "hdr_v2_deflate_b64": raw,
    })
}

// ---------------------------------------------------------------- throughput

pub fn throughput_run<E: Engine>(e: &E, ds: &Ds, d: Dur, readers: usize, secs: f64, seed: u64) -> Value {
    let stop = AtomicBool::new(false);
    let errors = AtomicU64::new(0);
    let errors = &errors;
    let wmix: Vec<(Op, u32)> = MIX.iter().copied().filter(|x| x.0.is_write()).collect();
    let rmix: Vec<(Op, u32)> = MIX.iter().copied().filter(|x| !x.0.is_write()).collect();
    let (wops, rops) = std::thread::scope(|sc| {
        let wh = sc.spawn(|| {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut c = OpCtx::new();
            let mut n = 0u64;
            let mut nw = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let op = pick(&mut rng, &wmix);
                run_op(e, ds, op, d, &mut rng, &mut c);
                maybe_checkpoint(e, d, op, &mut nw);
                n += 1;
            }
            n
        });
        let rhs: Vec<_> = (0..readers)
            .map(|i| {
                let stop = &stop;
                let rmix = &rmix;
                sc.spawn(move || {
                    let mut rng = SmallRng::seed_from_u64(seed + 1 + i as u64);
                    let mut c = OpCtx::new();
                    let mut n = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let op = pick(&mut rng, rmix);
                        // an engine error in a reader is counted, not fatal (seen with libmdbx)
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_op(e, ds, op, d, &mut rng, &mut c)));
                        if r.is_err() {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        std::thread::sleep(Duration::from_secs_f64(secs));
        stop.store(true, Ordering::Relaxed);
        let w = wh.join().unwrap();
        let r: u64 = rhs.into_iter().map(|h| h.join().unwrap()).sum();
        (w, r)
    });
    json!({"durability": format!("{:?}", d), "readers": readers, "secs": secs, "reader_errors": errors.load(Ordering::Relaxed),
        "writer_ops_per_s": wops as f64 / secs, "reader_ops_per_s": rops as f64 / secs,
        "reader_ops_per_s_per_thread": rops as f64 / secs / readers.max(1) as f64})
}

// ---------------------------------------------------------------- overwrite phase

/// `n` overwrites (50% property, 50% statement object), one per transaction, NoFlush,
/// with every `flush_every`-th commit Flush (a group-commit like cadence).
pub fn overwrite_phase<E: Engine>(e: &E, ds: &Ds, n: u64, flush_every: u64, seed: u64) -> Value {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut c = OpCtx::new();
    let (dw0, _) = disk_written();
    let t0 = Instant::now();
    for i in 0..n {
        let op = if rng.gen_bool(0.5) { Op::PropOverwrite } else { Op::ObjOverwrite };
        let d = if (i + 1) % flush_every == 0 || i + 1 == n { Dur::Flush } else { Dur::NoFlush };
        run_op(e, ds, op, d, &mut rng, &mut c);
    }
    let wall = t0.elapsed().as_secs_f64();
    let (dw1, _) = disk_written();
    json!({"overwrites": n, "flush_every": flush_every, "wall_s": wall,
        "disk_bytes_written": dw1 - dw0, "kv_bytes_changed": c.kv_bytes,
        "disk_bytes_per_overwrite": (dw1 - dw0) as f64 / n as f64,
        "write_amp_vs_kv_bytes": (dw1 - dw0) as f64 / c.kv_bytes.max(1) as f64})
}

// ---------------------------------------------------------------- cyclic queries

fn col(k: &[u8], i: usize) -> u64 {
    u64::from_be_bytes(k[i * 8..i * 8 + 8].try_into().unwrap())
}

/// Third components of all keys with a 16-byte prefix (a,b) — sorted by construction.
fn list2<R: ReadOps>(r: &R, t: Tab, a: u64, b: u64, out: &mut Vec<u64>) {
    out.clear();
    let mut p = [0u8; 16];
    p[0..8].copy_from_slice(&a.to_be_bytes());
    p[8..16].copy_from_slice(&b.to_be_bytes());
    scan_prefix(r, t, &p, |k, _| {
        out.push(col(k, 2));
        true
    });
}
/// Component `i` of all keys with an 8-byte prefix a.
fn list1<R: ReadOps>(r: &R, t: Tab, a: u64, i: usize, out: &mut Vec<u64>) {
    out.clear();
    scan_prefix(r, t, &a.to_be_bytes(), |k, _| {
        out.push(col(k, i));
        true
    });
}
fn isect(a: &[u64], b: &[u64]) -> u64 {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    Three,
    Six,
}

/// Q1 anchored triangle on one predicate: (a p b)(b p c)(c p a).
pub fn q_tri1<R: ReadOps>(r: &R, plan: Plan, a: u64, p: u64) -> u64 {
    let (mut bs, mut cin, mut cs) = (Vec::new(), Vec::new(), Vec::new());
    match plan {
        Plan::Three => {
            list2(r, Tab::Spo, a, p, &mut bs);
            list2(r, Tab::Pos, p, a, &mut cin);
        }
        Plan::Six => {
            list2(r, Tab::Pso, p, a, &mut bs);
            list2(r, Tab::Ops, a, p, &mut cin);
        }
    }
    let mut n = 0;
    for &b in &bs {
        match plan {
            Plan::Three => list2(r, Tab::Spo, b, p, &mut cs),
            Plan::Six => list2(r, Tab::Pso, p, b, &mut cs),
        }
        n += isect(&cs, &cin);
    }
    n
}

/// Q2 anchored 4-cycle on one predicate: (a p b)(b p c)(c p d)(d p a).
pub fn q_cyc4<R: ReadOps>(r: &R, plan: Plan, a: u64, p: u64) -> u64 {
    let (mut bs, mut din, mut cs, mut ds) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let fwd = |r: &R, x: u64, out: &mut Vec<u64>| match plan {
        Plan::Three => list2(r, Tab::Spo, x, p, out),
        Plan::Six => list2(r, Tab::Pso, p, x, out),
    };
    fwd(r, a, &mut bs);
    match plan {
        Plan::Three => list2(r, Tab::Pos, p, a, &mut din),
        Plan::Six => list2(r, Tab::Ops, a, p, &mut din),
    }
    let mut n = 0;
    for &b in &bs {
        fwd(r, b, &mut cs);
        for &c in &cs {
            fwd(r, c, &mut ds);
            n += isect(&ds, &din);
        }
    }
    n
}

/// Q3 anchored triangle, any predicates: (a ?p1 b)(b ?p2 c)(c ?p3 a).
/// Three orderings: objects of b come from SPO in predicate order -> sort + dedup.
/// Six orderings: SOP gives them in object order directly.
pub fn q_tri_any<R: ReadOps>(r: &R, plan: Plan, a: u64) -> u64 {
    let (mut bs, mut cin, mut cs) = (Vec::new(), Vec::new(), Vec::new());
    match plan {
        Plan::Three => {
            list1(r, Tab::Spo, a, 2, &mut bs);
            bs.sort_unstable();
        }
        Plan::Six => list1(r, Tab::Sop, a, 1, &mut bs),
    }
    bs.dedup();
    list1(r, Tab::Osp, a, 1, &mut cin); // (o=a, s=c, p): sorted by c
    cin.dedup();
    let mut n = 0;
    for &b in &bs {
        match plan {
            Plan::Three => {
                list1(r, Tab::Spo, b, 2, &mut cs);
                cs.sort_unstable();
            }
            Plan::Six => list1(r, Tab::Sop, b, 1, &mut cs),
        }
        cs.dedup();
        n += isect(&cs, &cin);
    }
    n
}

/// Q4 all triangles on one (rare) predicate p.
/// Three: iterate POS(p) grouped by a=o (gives Cin(a) = subjects pointing at a), then
/// SPO(a,p) and SPO(b,p). Six: iterate PSO(p) grouped by a=s, Cin from OPS(a,p), PSO(p,b).
pub fn q_tri_all<R: ReadOps>(r: &R, plan: Plan, p: u64) -> u64 {
    let mut groups: Vec<(u64, Vec<u64>)> = Vec::new();
    let t = if plan == Plan::Three { Tab::Pos } else { Tab::Pso };
    scan_prefix(r, t, &p.to_be_bytes(), |k, _| {
        let (g, x) = (col(k, 1), col(k, 2));
        match groups.last_mut() {
            Some((gg, v)) if *gg == g => v.push(x),
            _ => groups.push((g, vec![x])),
        }
        true
    });
    let (mut bs, mut cs, mut cin) = (Vec::new(), Vec::new(), Vec::new());
    let mut n = 0;
    for (a, v) in &groups {
        match plan {
            Plan::Three => {
                // v = subjects c with (c p a) = Cin(a)
                list2(r, Tab::Spo, *a, p, &mut bs);
                for &b in &bs {
                    list2(r, Tab::Spo, b, p, &mut cs);
                    n += isect(&cs, v);
                }
            }
            Plan::Six => {
                // v = objects b with (a p b)
                list2(r, Tab::Ops, *a, p, &mut cin);
                for &b in v {
                    list2(r, Tab::Pso, p, b, &mut cs);
                    n += isect(&cs, &cin);
                }
            }
        }
    }
    n
}

pub fn cyclic_suite<E: Engine>(e: &E, ds: &Ds, plan: Plan, nq: u64, seed: u64) -> Value {
    let mut rng = SmallRng::seed_from_u64(seed);
    let p_hot = pred_id(0);
    let p_rare = pred_id(NPRED - 1);
    let mut out = serde_json::Map::new();
    for (name, n) in [("tri1_anchored_p0", nq), ("cyc4_anchored_p0", nq), ("tri_anypred_anchored", nq), ("tri_all_rare_pred", 5)] {
        let mut h = new_hist();
        let mut results = 0u64;
        for _ in 0..n {
            let a = ds.rnode(&mut rng);
            let r = e.begin_read();
            let s = Instant::now();
            results += match name {
                "tri1_anchored_p0" => q_tri1(&r, plan, a, p_hot),
                "cyc4_anchored_p0" => q_cyc4(&r, plan, a, p_hot),
                "tri_anypred_anchored" => q_tri_any(&r, plan, a),
                _ => q_tri_all(&r, plan, p_rare),
            };
            h.record((s.elapsed().as_nanos() as u64).max(1)).unwrap();
        }
        let mut v = hist_summary(&h);
        v["results"] = json!(results);
        out.insert(name.into(), v);
    }
    json!({"plan": format!("{:?}", plan), "queries": out})
}

// ---------------------------------------------------------------- online ordering build

/// Build ordering `target` from SPO in batches of `batch` entries, one write txn per batch
/// (reads SPO inside the same txn, continues from the last key), NoFlush, final Flush.
pub fn build_ordering<E: Engine>(e: &E, target: Tab, batch: usize) -> Value {
    build_ordering_d(e, target, batch, Dur::Flush)
}

/// `batch_dur`: durability of each batch commit (the last one is always Flush). With redb,
/// NoFlush batches leak every rewritten page until a durable commit (measured: +18.6 GB for
/// one random-order ordering at 10M), so the default is Flush per batch.
pub fn build_ordering_d<E: Engine>(e: &E, target: Tab, batch: usize, batch_dur: Dur) -> Value {
    let files = e.files();
    let (len0, alloc0) = file_sizes(&files);
    let (dw0, _) = disk_written();
    let t0 = Instant::now();
    let mut lo = vec![0u8; 24];
    let hi = vec![0xFFu8; 24];
    let mut total = 0u64;
    let mut batches = 0u64;
    loop {
        let mut w = e.begin_write();
        let mut items: Vec<([u8; 24], [u8; 8])> = Vec::with_capacity(batch);
        let mut first = true;
        let skip_first = total > 0;
        w.scan(Tab::Spo, &lo, &hi, &mut |k, v| {
            if first && skip_first && k == &lo[..] {
                first = false;
                return true;
            }
            first = false;
            let (s, p, o) = (col(k, 0), col(k, 1), col(k, 2));
            items.push((target.key(s, p, o), v.try_into().unwrap()));
            if items.len() >= batch {
                return false;
            }
            true
        });
        if items.is_empty() {
            w.commit(Dur::Flush);
            break;
        }
        // remember last SPO key: recompute from the target key
        let last = items.last().unwrap().0;
        let (s, p, o) = match target {
            Tab::Pso => (col(&last, 1), col(&last, 0), col(&last, 2)),
            Tab::Sop => (col(&last, 0), col(&last, 2), col(&last, 1)),
            Tab::Ops => (col(&last, 2), col(&last, 1), col(&last, 0)),
            _ => unreachable!(),
        };
        lo = Tab::Spo.key(s, p, o).to_vec();
        total += items.len() as u64;
        batches += 1;
        let done = items.len() < batch;
        items.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in &items {
            w.insert(target, k, v);
        }
        w.commit(if done { Dur::Flush } else { batch_dur });
        if done {
            break;
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    let (dw1, _) = disk_written();
    let (len1, alloc1) = file_sizes(&files);
    json!({"target": target.name(), "batch": batch, "batch_durability": format!("{:?}", batch_dur), "entries": total, "batches": batches, "wall_s": wall,
        "file_len_before": len0, "file_len_after": len1, "alloc_before": alloc0, "alloc_after": alloc1,
        "growth_bytes_alloc": alloc1 as i64 - alloc0 as i64, "disk_bytes_written": dw1 - dw0})
}
