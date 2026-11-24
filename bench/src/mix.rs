//! The agent-memory mix ([[17-benchmarks]] intro) and its two drivers:
//! open-loop at a fixed arrival rate (latency, coordinated-omission safe) and
//! closed-loop (throughput).
//!
//! Mix `agent-mix-v1`, per mille of operations:
//! - 300 `props_get`   all literal properties of a node (point lookup)
//! - 300 `stmt_get`    a statement by its handle (point lookup)
//! - 150 `hop`         traversal of 1–3 hops (depth uniform), fan-out capped at
//!   16 per node and 64 per level; half inside a `GRAPH` scope
//! - 100 `star`        4–6 predicates of one node; half inside a scope (node
//!   predicates only when scoped, 2 literal + node predicates otherwise)
//! - 75 `write_txn`   1–20 statements: 70% value overwrites, 20% inserts of a
//!   new edge into the subject's episode, 10% deletes
//! - 25 `membership`  an existing fact into a second episode
//! - 50 `annotate`    a statement about a statement
//!
//! So writes are 15% of operations, a quarter of write transactions are
//! membership operations (rounded: 25 of 100 non-annotation writes), and half
//! of traversals and star patterns are scoped, as tg.md asks. A scope is the
//! node's home episode (50%) or that episode's agent, whose episodes are nested
//! under it. Scoped reads run on the adapter's own graph model (see each adapter).

use crate::adapter::{is_unsupported, Adapter, Dur, Scope, Session, Term, WOp};
use crate::gen::{ann_pred, episode_iri, lpred, node_iri, pred, Loaded, Rec, Shape, LPRED, NPRED};
use crate::stats::Hists;
use anyhow::Result;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MIX_VERSION: &str = "agent-mix-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Op {
    PropsGet,
    StmtGet,
    Hop,
    Star,
    WriteTxn,
    Membership,
    Annotate,
}

pub const MIX: [(Op, u32); 7] = [
    (Op::PropsGet, 300),
    (Op::StmtGet, 300),
    (Op::Hop, 150),
    (Op::Star, 100),
    (Op::WriteTxn, 75),
    (Op::Membership, 25),
    (Op::Annotate, 50),
];

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::PropsGet => "props_get",
            Op::StmtGet => "stmt_get",
            Op::Hop => "hop",
            Op::Star => "star",
            Op::WriteTxn => "write_txn",
            Op::Membership => "membership",
            Op::Annotate => "annotate",
        }
    }
    pub fn is_write(self) -> bool {
        matches!(self, Op::WriteTxn | Op::Membership | Op::Annotate)
    }
}

pub fn pick(rng: &mut SmallRng, mix: &[(Op, u32)]) -> Op {
    let tot: u32 = mix.iter().map(|x| x.1).sum();
    let mut r = rng.gen_range(0..tot);
    for &(op, w) in mix {
        if r < w {
            return op;
        }
        r -= w;
    }
    mix[mix.len() - 1].0
}

/// Mutable statement registry shared by the workers of one adapter.
#[derive(Debug)]
pub struct State {
    pub shape: Shape,
    pub recs: Mutex<Vec<Rec>>,
}

impl State {
    pub fn new(l: Loaded) -> State {
        State {
            shape: l.shape,
            recs: Mutex::new(l.recs),
        }
    }
    fn rand_live(&self, rng: &mut SmallRng) -> Option<(usize, Rec)> {
        let recs = self.recs.lock().expect("registry");
        if recs.is_empty() {
            return None;
        }
        for _ in 0..8 {
            let i = rng.gen_range(0..recs.len());
            let r = recs[i];
            if r.live {
                return Some((i, r));
            }
        }
        None
    }
    fn scope(&self, rng: &mut SmallRng, node: u64) -> Scope {
        let ep = self.shape.episode_of(node);
        if rng.gen_bool(0.5) {
            Scope {
                graph: episode_iri(ep),
                episodes: vec![episode_iri(ep)],
            }
        } else {
            let a = self.shape.agent_of_episode(ep);
            Scope {
                graph: crate::gen::agent_iri(a),
                episodes: self.shape.episodes_of_agent(a),
            }
        }
    }
}

/// Per-class counters beyond latency.
#[derive(Debug, Default, Clone)]
pub struct Counters {
    pub rows: u64,
    pub errors: BTreeMap<&'static str, u64>,
    pub unsupported: BTreeMap<&'static str, u64>,
    pub first_error: Option<String>,
}

impl Counters {
    fn merge(&mut self, o: &Counters) {
        self.rows += o.rows;
        for (k, v) in &o.errors {
            *self.errors.entry(k).or_default() += v;
        }
        for (k, v) in &o.unsupported {
            *self.unsupported.entry(k).or_default() += v;
        }
        if self.first_error.is_none() {
            self.first_error.clone_from(&o.first_error);
        }
    }
}

/// Runs one operation; returns rows touched.
pub fn run_op(s: &mut dyn Session, st: &State, op: Op, dur: Dur, rng: &mut SmallRng) -> Result<u64> {
    let nodes = st.shape.nodes;
    match op {
        Op::PropsGet => {
            let n = rng.gen_range(0..nodes);
            Ok(match s.node(&node_iri(n))? {
                Some(h) => s.props(h)? as u64,
                None => 0,
            })
        }
        Op::StmtGet => match st.rand_live(rng) {
            Some((_, r)) => Ok(s.stmt(&r.to_ref())? as u64),
            None => Ok(0),
        },
        Op::Hop => {
            let n = rng.gen_range(0..nodes);
            let depth = rng.gen_range(1..=3);
            let scope = rng.gen_bool(0.5).then(|| st.scope(rng, n));
            let Some(h) = s.node(&node_iri(n))? else { return Ok(0) };
            let mut frontier = vec![h];
            let mut rows = 0u64;
            for _ in 0..depth {
                let mut next = Vec::new();
                for &f in &frontier {
                    let o = s.out(f, 16, scope.as_ref())?;
                    rows += o.len() as u64;
                    next.extend(o);
                    if next.len() >= 64 {
                        next.truncate(64);
                        break;
                    }
                }
                frontier = next;
                if frontier.is_empty() {
                    break;
                }
            }
            Ok(rows)
        }
        Op::Star => {
            let n = rng.gen_range(0..nodes);
            let k = rng.gen_range(4..=6);
            let scoped = rng.gen_bool(0.5);
            let mut preds: Vec<Term> = Vec::with_capacity(k);
            if !scoped {
                for j in 0..2 {
                    preds.push(lpred((rng.gen_range(0..LPRED / 2) * 2 + j) % LPRED));
                }
            }
            while preds.len() < k {
                // Low predicate numbers are the frequent ones (skewed generator).
                preds.push(pred(rng.gen_range(0..NPRED / 4)));
            }
            let scope = scoped.then(|| st.scope(rng, n));
            let Some(h) = s.node(&node_iri(n))? else { return Ok(0) };
            Ok(s.star(h, &preds, scope.as_ref())? as u64)
        }
        Op::WriteTxn => {
            let k = rng.gen_range(1..=20);
            let mut ops = Vec::with_capacity(k);
            let mut new_recs = Vec::new();
            let mut deleted = Vec::new();
            for _ in 0..k {
                let x: f64 = rng.gen();
                if x < 0.7 {
                    let n = rng.gen_range(0..nodes);
                    ops.push(WOp::Overwrite {
                        s: Term::Iri(node_iri(n)),
                        p: lpred(rng.gen_range(0..LPRED)),
                        o: Term::int(rng.gen_range(0..1_000_000)),
                    });
                } else if x < 0.9 {
                    let sn = rng.gen_range(0..nodes);
                    let (p, o) = (crate::gen::rand_pred(rng), crate::gen::rand_obj(rng, nodes));
                    let ep = st.shape.episode_of(sn);
                    ops.push(WOp::Insert {
                        s: Term::Iri(node_iri(sn)),
                        p: pred(p),
                        o: Term::Iri(node_iri(o)),
                        g: Some(episode_iri(ep)),
                    });
                    new_recs.push(Rec {
                        native: None,
                        s: sn,
                        p,
                        o,
                        ep,
                        live: true,
                    });
                } else if let Some((i, r)) = st.rand_live(rng) {
                    if !deleted.contains(&i) {
                        deleted.push(i);
                        ops.push(WOp::Delete(r.to_ref()));
                    }
                }
            }
            let ids = s.write(&ops, dur)?;
            let mut recs = st.recs.lock().expect("registry");
            for (mut r, id) in new_recs.into_iter().zip(ids) {
                r.native = id;
                recs.push(r);
            }
            for i in deleted {
                recs[i].live = false;
            }
            Ok(k as u64)
        }
        Op::Membership => {
            let Some((_, r)) = st.rand_live(rng) else { return Ok(0) };
            let mut e2 = rng.gen_range(0..st.shape.episodes);
            if e2 == r.ep && st.shape.episodes > 1 {
                e2 = (e2 + 1) % st.shape.episodes;
            }
            s.write(
                &[WOp::AddMember {
                    target: r.to_ref(),
                    g: episode_iri(e2),
                }],
                dur,
            )?;
            Ok(1)
        }
        Op::Annotate => {
            let Some((_, r)) = st.rand_live(rng) else { return Ok(0) };
            s.write(
                &[WOp::Annotate {
                    target: r.to_ref(),
                    p: ann_pred(rng.gen_range(0..8)),
                    o: Term::int(rng.gen_range(0..100)),
                }],
                dur,
            )?;
            Ok(1)
        }
    }
}

/// Driver settings.
#[derive(Clone, Debug)]
pub struct DriveCfg {
    pub ops: u64,
    pub warmup: u64,
    pub threads: usize,
    /// Some(rate): open loop at `rate` ops/s total; None: closed loop.
    pub rate: Option<f64>,
    pub dur: Dur,
    pub seed: u64,
    pub mix: Vec<(Op, u32)>,
}

/// Result of one run.
#[derive(Debug)]
pub struct RunOut {
    /// Latency from intended start (open loop) or from actual start (closed loop).
    pub latency: Hists,
    /// Service time (actual start to completion), both modes.
    pub service: Hists,
    pub counters: Counters,
    pub wall_s: f64,
    pub ops: u64,
    pub disk_bytes_written: Option<u64>,
    pub max_behind_us: f64,
}

impl RunOut {
    pub fn to_json(&self) -> Value {
        json!({
            "wall_s": self.wall_s,
            "ops": self.ops,
            "ops_per_s": self.ops as f64 / self.wall_s.max(1e-9),
            "latency": self.latency.to_json(),
            "service_time": self.service.to_json(),
            "rows": self.counters.rows,
            "errors": self.counters.errors,
            "unsupported": self.counters.unsupported,
            "first_error": self.counters.first_error,
            "disk_bytes_written": self.disk_bytes_written,
            "max_schedule_lag_us": self.max_behind_us,
        })
    }
}

fn wait_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let d = t - now;
        if d > Duration::from_micros(200) {
            std::thread::sleep(d - Duration::from_micros(100));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Runs warm-up then `ops` operations with `threads` workers.
///
/// Open loop: operation i is scheduled at `t0 + i/rate` regardless of how long
/// earlier operations took; its latency is completion minus scheduled time,
/// so a stall is charged to every operation queued behind it (no coordinated
/// omission). Closed loop: each worker issues back to back; latency equals
/// service time and the run reports throughput.
pub fn drive(a: &dyn Adapter, st: &State, cfg: &DriveCfg) -> Result<RunOut> {
    // Warm-up, closed loop, not recorded.
    {
        let mut s = a.session()?;
        let mut rng = SmallRng::seed_from_u64(cfg.seed ^ 0xA5A5);
        for _ in 0..cfg.warmup {
            let op = pick(&mut rng, &cfg.mix);
            let _ = run_op(s.as_mut(), st, op, cfg.dur, &mut rng);
        }
    }
    let next = AtomicU64::new(0);
    let behind = AtomicU64::new(0);
    let d0 = crate::env::disk_bytes_written();
    let t0 = Instant::now() + Duration::from_millis(5);
    let results: Vec<Result<(Hists, Hists, Counters)>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..cfg.threads)
            .map(|w| {
                let next = &next;
                let behind = &behind;
                sc.spawn(move || -> Result<(Hists, Hists, Counters)> {
                    let mut s = a.session()?;
                    let mut rng = SmallRng::seed_from_u64(cfg.seed.wrapping_add(1 + w as u64));
                    let (mut lat, mut svc, mut c) = (Hists::default(), Hists::default(), Counters::default());
                    let per = cfg.ops / cfg.threads as u64 + u64::from((w as u64) < cfg.ops % cfg.threads as u64);
                    let mut done = 0;
                    loop {
                        let intended = match cfg.rate {
                            Some(r) => {
                                let i = next.fetch_add(1, Ordering::Relaxed);
                                if i >= cfg.ops {
                                    break;
                                }
                                let t = t0 + Duration::from_secs_f64(i as f64 / r);
                                wait_until(t);
                                let lag = Instant::now().saturating_duration_since(t).as_micros() as u64;
                                behind.fetch_max(lag, Ordering::Relaxed);
                                t
                            }
                            None => {
                                if done >= per {
                                    break;
                                }
                                done += 1;
                                Instant::now()
                            }
                        };
                        let op = pick(&mut rng, &cfg.mix);
                        let start = Instant::now();
                        let r = run_op(s.as_mut(), st, op, cfg.dur, &mut rng);
                        let end = Instant::now();
                        lat.rec(op.name(), (end - intended).as_nanos() as u64);
                        svc.rec(op.name(), (end - start).as_nanos() as u64);
                        match r {
                            Ok(rows) => c.rows += rows,
                            Err(e) if is_unsupported(&e) => *c.unsupported.entry(op.name()).or_default() += 1,
                            Err(e) => {
                                *c.errors.entry(op.name()).or_default() += 1;
                                if c.first_error.is_none() {
                                    c.first_error = Some(format!("{}: {e:#}", op.name()));
                                }
                            }
                        }
                    }
                    Ok((lat, svc, c))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("worker")).collect()
    });
    let wall = t0.elapsed().as_secs_f64();
    let d1 = crate::env::disk_bytes_written();
    let mut out = RunOut {
        latency: Hists::default(),
        service: Hists::default(),
        counters: Counters::default(),
        wall_s: wall,
        ops: cfg.ops,
        disk_bytes_written: d0.zip(d1).map(|(a, b)| b.saturating_sub(a)),
        max_behind_us: behind.load(Ordering::Relaxed) as f64,
    };
    for r in results {
        let (l, s, c) = r?;
        out.latency.merge(&l);
        out.service.merge(&s);
        out.counters.merge(&c);
    }
    Ok(out)
}

/// The read-only part of the mix (reader threads).
pub fn read_mix() -> Vec<(Op, u32)> {
    MIX.iter().copied().filter(|x| !x.0.is_write()).collect()
}

/// The write part of the mix (the writer of reader-scaling runs).
pub fn write_mix() -> Vec<(Op, u32)> {
    MIX.iter().copied().filter(|x| x.0.is_write()).collect()
}
