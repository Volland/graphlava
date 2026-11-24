//! Dataset generator (v1 encoding, tg.md/02-id-encoding.md bit layout) and bulk loader.
//!
//! Dataset `gen-v1`, parameters derived from N (node-valued statements):
//! - nodes = N/8 IRIs (kind 0x1, sequential), subject out-degree uniform 4..=12 (mean 8),
//!   generated subject by subject; duplicate (p,o) per subject removed.
//! - 64 node predicates (IRIs), skewed: p = floor(64*u^2) (p0 ~12.5%, p63 ~0.8%).
//! - object: 50% uniform node, 50% hub-skewed floor(nodes*u^3).
//! - statement IDs kind 0x4 chunked: chunk = (k>>20)+1, offset = k & 0xFFFFF.
//! - every node has 4 inline integer properties (16 literal predicates), literal IDs
//!   0xC<<60 | (2^57 + v) (class 11, subtype 0), stored as one `props` record.

use crate::engine::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

pub const NPRED: u64 = 64;
pub const LPRED: u64 = 16;
pub const PROPS_PER_NODE: usize = 4;

pub fn iri(seq: u64) -> u64 {
    (1u64 << 60) | seq
}
pub fn node_id(n: u64) -> u64 {
    iri(1000 + n)
}
pub fn pred_id(j: u64) -> u64 {
    iri(1 + j)
}
pub fn stmt_id(k: u64) -> u64 {
    (4u64 << 60) | (((k >> 20) + 1) << 20) | (k & 0xFFFFF)
}
pub fn int_lit(v: u64) -> u64 {
    (0xCu64 << 60) | ((1u64 << 57) + v)
}

pub fn rand_pred(rng: &mut SmallRng) -> u64 {
    let u: f64 = rng.gen();
    pred_id(((NPRED as f64) * u * u) as u64 % NPRED)
}
pub fn rand_obj(rng: &mut SmallRng, nodes: u64) -> u64 {
    if rng.gen_bool(0.5) {
        node_id(rng.gen_range(0..nodes))
    } else {
        let u: f64 = rng.gen();
        node_id(((nodes as f64) * u * u * u) as u64 % nodes)
    }
}

pub fn stmt_val(s: u64, p: u64, o: u64, flags: u16) -> [u8; 26] {
    let mut v = [0u8; 26];
    v[0..8].copy_from_slice(&s.to_le_bytes());
    v[8..16].copy_from_slice(&p.to_le_bytes());
    v[16..24].copy_from_slice(&o.to_le_bytes());
    v[24..26].copy_from_slice(&flags.to_le_bytes());
    v
}
pub fn stmt_decode(v: &[u8]) -> (u64, u64, u64) {
    (
        u64::from_le_bytes(v[0..8].try_into().unwrap()),
        u64::from_le_bytes(v[8..16].try_into().unwrap()),
        u64::from_le_bytes(v[16..24].try_into().unwrap()),
    )
}

pub fn props_record(rng: &mut SmallRng) -> Vec<u8> {
    let mut preds: Vec<u64> = Vec::with_capacity(PROPS_PER_NODE);
    while preds.len() < PROPS_PER_NODE {
        let p = pred_id(NPRED + rng.gen_range(0..LPRED));
        if !preds.contains(&p) {
            preds.push(p);
        }
    }
    preds.sort();
    let mut r = Vec::with_capacity(4 + 16 * PROPS_PER_NODE);
    r.push(1u8);
    r.push(0u8);
    r.extend_from_slice(&(PROPS_PER_NODE as u16).to_le_bytes());
    for p in preds {
        r.extend_from_slice(&p.to_le_bytes());
        r.extend_from_slice(&int_lit(rng.gen_range(0..1_000_000)).to_le_bytes());
    }
    r
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DsMeta {
    pub n: u64,
    pub nodes: u64,
    pub loaded_stmts: u64,
    pub next_stmt: u64,
    pub seed: u64,
    pub ords: Vec<String>,
}

pub fn ords_from_names(v: &[String]) -> Vec<Tab> {
    v.iter()
        .map(|n| *Tab::ORD.iter().find(|t| t.name() == n).unwrap())
        .collect()
}

/// Bulk load: batches of `batch` statements, each batch sorted per table, one write
/// transaction per batch committed with `batch_dur` (default Flush: redb frees COW pages
/// only on durable commits, so NoFlush batches make the file grow with every rewritten
/// page); the last batch always commits Flush.
pub fn bulk_load<E: Engine>(e: &E, n: u64, ords: &[Tab], seed: u64, batch: u64, verbose: bool) -> DsMeta {
    bulk_load_d(e, n, ords, seed, batch, verbose, Dur::Flush)
}

pub fn bulk_load_d<E: Engine>(e: &E, n: u64, ords: &[Tab], seed: u64, batch: u64, verbose: bool, batch_dur: Dur) -> DsMeta {
    let nodes = (n / 8).max(16);
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut k: u64 = 0;
    let mut node: u64 = 0;
    let mut ordv: Vec<Vec<([u8; 24], u64)>> = vec![Vec::new(); ords.len()];
    let mut stmtv: Vec<(u64, [u8; 26])> = Vec::new();
    let mut propv: Vec<(u64, Vec<u8>)> = Vec::new();
    let t0 = std::time::Instant::now();
    let mut pending = 0u64;
    let mut nbatch = 0u64;
    let mut edges: Vec<(u64, u64)> = Vec::with_capacity(64);
    while node < nodes {
        let s = node_id(node);
        let deg = if node == nodes - 1 {
            n.saturating_sub(k)
        } else {
            rng.gen_range(4..=12u64).min(n.saturating_sub(k))
        };
        edges.clear();
        for _ in 0..deg {
            edges.push((rand_pred(&mut rng), rand_obj(&mut rng, nodes)));
        }
        edges.sort_unstable();
        edges.dedup();
        for &(p, o) in &edges {
            let id = stmt_id(k);
            k += 1;
            for (i, t) in ords.iter().enumerate() {
                ordv[i].push((t.key(s, p, o), id));
            }
            stmtv.push((id, stmt_val(s, p, o, 0)));
            pending += 1;
        }
        propv.push((s, props_record(&mut rng)));
        node += 1;
        if pending >= batch || node == nodes {
            let last = node == nodes;
            let mut w = e.begin_write();
            for (i, t) in ords.iter().enumerate() {
                let v = &mut ordv[i];
                v.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                for (key, id) in v.iter() {
                    w.insert(*t, key, &id.to_le_bytes());
                }
                v.clear();
            }
            for (id, val) in stmtv.drain(..) {
                w.insert(Tab::Stmt, &id.to_be_bytes(), &val);
            }
            for (s, rec) in propv.drain(..) {
                w.insert(Tab::Props, &s.to_be_bytes(), &rec);
            }
            w.commit(if last { Dur::Flush } else { batch_dur });
            nbatch += 1;
            pending = 0;
            if verbose && (nbatch % 8 == 0 || last) {
                eprintln!("  load {}: {} stmts, {:.1}s", e.name(), k, t0.elapsed().as_secs_f64());
            }
        }
    }
    DsMeta {
        n,
        nodes,
        loaded_stmts: k,
        next_stmt: k,
        seed,
        ords: ords.iter().map(|t| t.name().to_string()).collect(),
    }
}
