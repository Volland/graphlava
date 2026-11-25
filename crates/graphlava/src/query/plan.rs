//! Planner ([[14-planner-execution]]): statistics, join ordering, and the
//! plan cache with drift invalidation ([[adr:0041]]).
//!
//! Predicate counts are exact and kept by the writer at commit time (rule
//! `plan-stats-no-block`); characteristic sets are computed by
//! [`analyze`] (the background pass of rule `plan-charsets`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use crate::error::Result;
use crate::id::Id;
use crate::query::exec::{Exec, Row};
use crate::query::ir::*;
use crate::storage::{KvRead, KvWrite, prefix_iter};
use crate::store;
use crate::tables::{self as tb, Perm};

/// Default drift factor that invalidates a cached plan (rule
/// `plan-cache-invalidate`).
pub const DRIFT: f64 = 4.0;

fn pred_key(p: Id) -> String {
    format!("stat:p:{:016x}", p.raw())
}

/// Adds `delta` to the statement count of predicate `p` (called by the
/// writer for every statement and inline property it adds or removes).
pub fn bump(kv: &mut dyn KvWrite, p: Id, delta: i64) -> Result<()> {
    let k = pred_key(p);
    let n = tb::meta_u64(kv.as_read(), &k)?.unwrap_or(0) as i64 + delta;
    if n <= 0 {
        kv.del(&tb::META, k.as_bytes())?;
    } else {
        tb::meta_put_u64(kv, &k, n as u64)?;
    }
    let t = tb::meta_u64(kv.as_read(), "stat:n")?.unwrap_or(0) as i64 + delta;
    tb::meta_put_u64(kv, "stat:n", t.max(0) as u64)
}

/// The statistics a plan reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub total: u64,
    pub pred: HashMap<Id, u64>,
    /// Characteristic sets: sorted predicate set → number of subjects.
    pub charsets: Vec<(Vec<Id>, u64)>,
    /// Sizes of the Datalog relations the executor holds right now,
    /// (full, delta) by name; filled at run time, not persisted.
    pub rels: HashMap<String, (usize, usize)>,
    /// State of each on-demand ordering (0 absent, 1 building, 2 ready).
    pub orderings: [u64; 3],
}

impl Stats {
    pub fn load(kv: &dyn KvRead) -> Result<Stats> {
        let mut st = Stats {
            total: tb::meta_u64(kv, "stat:n")?.unwrap_or(0),
            ..Stats::default()
        };
        for e in prefix_iter(kv, &tb::META, b"stat:p:") {
            let (k, v) = e?;
            let p =
                u64::from_str_radix(std::str::from_utf8(&k[7..]).unwrap_or("0"), 16).unwrap_or(0);
            st.pred
                .insert(Id(p), u64::from_le_bytes(v[..8].try_into().expect("8")));
        }
        if let Some(v) = tb::meta_get(kv, "stat:charsets")? {
            st.charsets = decode_charsets(&v);
        }
        for (i, p) in Perm::ON_DEMAND.iter().enumerate() {
            st.orderings[i] = store::ordering_state(kv, *p)?;
        }
        Ok(st)
    }

    pub fn count(&self, p: Id) -> u64 {
        self.pred.get(&p).copied().unwrap_or(0)
    }

    /// Subjects whose characteristic set contains all of `preds` (rule
    /// `plan-charsets`); `None` without a characteristic-set pass.
    pub fn star(&self, preds: &[Id]) -> Option<u64> {
        if self.charsets.is_empty() {
            return None;
        }
        Some(
            self.charsets
                .iter()
                .filter(|(set, _)| preds.iter().all(|p| set.binary_search(p).is_ok()))
                .map(|(_, n)| *n)
                .sum(),
        )
    }

    /// Whether `other` drifted from this snapshot by more than the factor on
    /// any predicate in `preds`, or the set of orderings changed.
    pub fn drifted(&self, other: &Stats, preds: &[Id], factor: f64) -> bool {
        if self.orderings != other.orderings {
            return true;
        }
        let ratio = |a: u64, b: u64| {
            let (a, b) = (a.max(1) as f64, b.max(1) as f64);
            if a > b { a / b } else { b / a }
        };
        ratio(self.total, other.total) > factor
            || preds
                .iter()
                .any(|p| ratio(self.count(*p), other.count(*p)) > factor)
    }
}

fn encode_charsets(cs: &[(Vec<Id>, u64)]) -> Vec<u8> {
    let mut v = Vec::new();
    for (set, n) in cs {
        v.extend_from_slice(&(set.len() as u32).to_le_bytes());
        for p in set {
            v.extend_from_slice(&p.le_bytes());
        }
        v.extend_from_slice(&n.to_le_bytes());
    }
    v
}

fn decode_charsets(v: &[u8]) -> Vec<(Vec<Id>, u64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= v.len() {
        let n = u32::from_le_bytes(v[i..i + 4].try_into().expect("4")) as usize;
        i += 4;
        let set = (0..n).map(|k| Id::from_le(&v[i + 8 * k..])).collect();
        i += 8 * n;
        let c = u64::from_le_bytes(v[i..i + 8].try_into().expect("8"));
        i += 8;
        out.push((set, c));
    }
    out
}

/// The background statistics pass: one scan of SPO and the property records
/// computes the characteristic sets (rule `plan-charsets`).
pub fn analyze(kv: &dyn KvRead) -> Result<Vec<(Vec<Id>, u64)>> {
    let mut per_subject: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for m in store::find_stored(kv, None, None, None)? {
        per_subject.entry(m.s).or_default().push(m.p);
    }
    let mut counts: HashMap<Vec<Id>, u64> = HashMap::new();
    for (_, mut ps) in per_subject {
        ps.sort();
        ps.dedup();
        *counts.entry(ps).or_default() += 1;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort();
    Ok(v)
}

pub fn store_charsets(kv: &mut dyn KvWrite, cs: &[(Vec<Id>, u64)]) -> Result<()> {
    tb::meta_put(kv, "stat:charsets", &encode_charsets(cs))
}

// ---- join ordering (rule `plan-join-choice`) ------------------------------------

/// Estimated rows of one join part when `bound` slots are already bound.
fn estimate(
    part: &Node,
    bound: &HashSet<Slot>,
    stats: &Stats,
    star: &HashMap<Slot, Vec<Id>>,
) -> f64 {
    let is_bound = |p: &Pt| match p {
        Pt::Const(_) => true,
        Pt::Var(v) => bound.contains(v),
    };
    match part {
        Node::Triple(t) => {
            if let Some(Pt::Var(r)) = &t.id {
                if bound.contains(r) {
                    return 1.0;
                }
            }
            let (s, p, o) = (is_bound(&t.s), is_bound(&t.p), is_bound(&t.o));
            let pc = match t.p {
                Pt::Const(c) => stats.count(c) as f64,
                Pt::Var(_) => stats.total as f64,
            }
            .max(1.0);
            match (s, p, o) {
                (true, _, true) => 1.0,
                (true, true, false) => (pc / 1000.0).clamp(1.0, 50.0),
                (true, false, false) => 20.0,
                (false, true, true) => (pc / 100.0).clamp(1.0, 1000.0),
                (false, false, true) => 50.0,
                (false, true, false) => {
                    // A star on an unbound subject: characteristic sets.
                    let preds = match t.s {
                        Pt::Var(v) => star.get(&v),
                        Pt::Const(_) => None,
                    };
                    match preds {
                        Some(preds) if preds.len() > 1 => {
                            stats.star(preds).map_or(pc, |n| n as f64).min(pc)
                        }
                        _ => pc,
                    }
                }
                (false, false, false) => (stats.total as f64).max(1.0) * 10.0,
            }
        }
        Node::Path { s, o, .. } => {
            if is_bound(s) || is_bound(o) {
                100.0
            } else {
                1e9
            }
        }
        Node::Values(_, rows) => rows.len() as f64,
        Node::Rel {
            name,
            args,
            version,
        } => {
            let (full, delta) = stats.rels.get(name).copied().unwrap_or((0, 0));
            let size = match version {
                Version::Full => full,
                Version::Delta => delta,
            } as f64;
            if args.iter().any(is_bound) {
                (size / 10.0).max(1.0)
            } else {
                size.max(1.0)
            }
        }
        Node::Datalog { args, .. } => {
            if args.iter().any(is_bound) {
                100.0
            } else {
                1e6
            }
        }
        Node::NamedGraphs(_) => 10.0,
        Node::Unit => 1.0,
        Node::Empty => 0.0,
        _ => 1e7,
    }
}

/// Orders join parts greedily: each step takes the cheapest part, preferring
/// parts connected to what is already bound (no cartesian products while a
/// connected part exists). Parts that are not sideways-safe go last.
pub fn order_parts(parts: &[Node], bound0: &HashSet<Slot>, stats: &Stats) -> Vec<usize> {
    let mut star: HashMap<Slot, Vec<Id>> = HashMap::new();
    for p in parts {
        if let Node::Triple(TriplePat {
            s: Pt::Var(v),
            p: Pt::Const(c),
            ..
        }) = p
        {
            star.entry(*v).or_default().push(*c);
        }
    }
    for v in star.values_mut() {
        v.sort();
        v.dedup();
    }
    let mut bound = bound0.clone();
    let mut left: Vec<usize> = (0..parts.len()).collect();
    let mut order = Vec::with_capacity(parts.len());
    while !left.is_empty() {
        let connected = |i: &usize| {
            let mut vs = Vec::new();
            parts[*i].bound_slots(&mut vs);
            bound.is_empty() || vs.iter().any(|v| bound.contains(v)) || vs.is_empty()
        };
        let safe: Vec<usize> = left
            .iter()
            .copied()
            .filter(|i| parts[*i].sideways_safe())
            .collect();
        let pool: Vec<usize> = if safe.is_empty() { left.clone() } else { safe };
        let conn: Vec<usize> = pool.iter().copied().filter(connected).collect();
        let pool = if conn.is_empty() { pool } else { conn };
        let best = *pool
            .iter()
            .min_by(|a, b| {
                estimate(&parts[**a], &bound, stats, &star)
                    .partial_cmp(&estimate(&parts[**b], &bound, stats, &star))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("non-empty pool");
        order.push(best);
        left.retain(|i| *i != best);
        let mut vs = Vec::new();
        parts[best].bound_slots(&mut vs);
        bound.extend(vs);
    }
    order
}

/// Runtime ordering for a join over the given input rows.
pub fn order_join(ex: &Exec<'_>, parts: &[Node], input: &[Row]) -> Vec<usize> {
    let bound: HashSet<Slot> = match input.first() {
        Some(r) => (0..r.len())
            .filter(|s| input.iter().all(|x| !x[*s].is_null()))
            .collect(),
        None => HashSet::new(),
    };
    let mut stats = ex.stats.clone().unwrap_or_default();
    if parts.iter().any(|p| matches!(p, Node::Rel { .. })) {
        stats.rels = ex
            .rels
            .borrow()
            .iter()
            .map(|(k, (f, d))| (k.clone(), (f.len(), d.len())))
            .collect();
    }
    order_parts(parts, &bound, &stats)
}

/// Predicates a plan reads, for drift checks.
pub fn plan_preds(n: &Node, out: &mut Vec<Id>) {
    match n {
        Node::Triple(TriplePat {
            p: Pt::Const(c), ..
        }) => out.push(*c),
        Node::Join(v) | Node::Union(v) => v.iter().for_each(|x| plan_preds(x, out)),
        Node::LeftJoin(a, b, _) | Node::Minus(a, b) | Node::AntiJoin(a, b) => {
            plan_preds(a, out);
            plan_preds(b, out);
        }
        Node::Filter(_, a)
        | Node::Extend(a, _, _)
        | Node::Distinct(a)
        | Node::OrderBy(a, _)
        | Node::Slice(a, _, _)
        | Node::Project(a, _) => plan_preds(a, out),
        Node::Aggregate { input, .. } => plan_preds(input, out),
        Node::PerGraph { inner, .. } => plan_preds(inner, out),
        _ => {}
    }
}

// ---- plan cache (rules `plan-cache-key`, `plan-cache-invalidate`) ---------------

/// A cached compilation: the planned IR plus the statistics it was planned
/// with.
#[derive(Debug, Clone)]
pub struct Cached<T> {
    pub plan: T,
    pub stats: Stats,
    pub preds: Vec<Id>,
}

/// The owner process's plan cache, keyed by normalised query text and mode.
#[derive(Debug, Default)]
pub struct PlanCache<T: Clone> {
    map: Mutex<HashMap<String, Cached<T>>>,
    pub hits: std::sync::atomic::AtomicU64,
    pub misses: std::sync::atomic::AtomicU64,
    pub invalidations: std::sync::atomic::AtomicU64,
}

impl<T: Clone> PlanCache<T> {
    pub fn new() -> Self {
        PlanCache {
            map: Mutex::new(HashMap::new()),
            hits: Default::default(),
            misses: Default::default(),
            invalidations: Default::default(),
        }
    }

    /// The cached plan for `key` unless the statistics drifted.
    pub fn get(&self, key: &str, now: &Stats) -> Option<T> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        match m.get(key) {
            Some(c) if !c.stats.drifted(now, &c.preds, DRIFT) => {
                self.hits.fetch_add(1, Relaxed);
                Some(c.plan.clone())
            }
            Some(_) => {
                m.remove(key);
                self.invalidations.fetch_add(1, Relaxed);
                self.misses.fetch_add(1, Relaxed);
                None
            }
            None => {
                self.misses.fetch_add(1, Relaxed);
                None
            }
        }
    }

    pub fn put(&self, key: String, plan: T, stats: Stats, preds: Vec<Id>) {
        let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if m.len() > 1024 {
            m.clear();
        }
        m.insert(key, Cached { plan, stats, preds });
    }

    /// Drops every plan (an ordering, value index or rule changed).
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
