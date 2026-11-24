//! Seeded, versioned dataset generator for the agent-memory mix.
//!
//! Dataset `glv-agent-v1` (derived from `spikes/bakeoff/src/data.rs` gen-v1,
//! re-expressed as terms instead of raw IDs), parameters from N, the number of
//! node-valued statements:
//! - nodes = max(N/8, 16) IRIs; subject out-degree uniform 4..=12 (mean 8),
//!   generated subject by subject, duplicate (p, o) per subject removed;
//! - 64 node predicates, skewed: p = floor(64·u²);
//! - object: 50% uniform node, 50% hub-skewed floor(nodes·u³);
//! - every node has 4 integer properties drawn from 16 literal predicates,
//!   values uniform in 0..1_000_000;
//! - episodes = max(nodes/64, 1) named graphs; every statement of a subject
//!   goes to the subject's home episode `ep(n) = (n·2654435761) mod episodes`;
//!   agents = max(episodes/16, 1); episode e is nested under agent e mod agents.
//!
//! The stream is a pure function of (N, seed); bump [`GEN_VERSION`] on any change.

use crate::adapter::{Adapter, Dur, StmtRef, Term, WOp};
use anyhow::Result;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use std::time::Instant;

pub const GEN_VERSION: &str = "glv-agent-v1";
pub const BASE: &str = "http://bench.graphlava.org/";
pub const NPRED: u64 = 64;
pub const LPRED: u64 = 16;
pub const PROPS_PER_NODE: usize = 4;

pub fn node_iri(i: u64) -> String {
    format!("{BASE}n/{i}")
}
pub fn node_index(iri: &str) -> Option<u64> {
    iri.strip_prefix(BASE)?.strip_prefix("n/")?.parse().ok()
}
pub fn pred(j: u64) -> Term {
    Term::Iri(format!("{BASE}p/{j}"))
}
pub fn lpred(j: u64) -> Term {
    Term::Iri(format!("{BASE}lp/{j}"))
}
pub fn episode_iri(e: u64) -> String {
    format!("{BASE}episode/{e}")
}
pub fn agent_iri(a: u64) -> String {
    format!("{BASE}agent/{a}")
}
pub fn ann_pred(j: u64) -> Term {
    Term::Iri(format!("{BASE}ann/{j}"))
}

pub fn rand_pred(rng: &mut SmallRng) -> u64 {
    let u: f64 = rng.gen();
    ((NPRED as f64) * u * u) as u64 % NPRED
}

pub fn rand_obj(rng: &mut SmallRng, nodes: u64) -> u64 {
    if rng.gen_bool(0.5) {
        rng.gen_range(0..nodes)
    } else {
        let u: f64 = rng.gen();
        ((nodes as f64) * u * u * u) as u64 % nodes
    }
}

/// Shape of a generated dataset.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Shape {
    pub n: u64,
    pub seed: u64,
    pub nodes: u64,
    pub episodes: u64,
    pub agents: u64,
}

impl Shape {
    pub fn new(n: u64, seed: u64) -> Shape {
        let nodes = (n / 8).max(16);
        let episodes = (nodes / 64).max(1);
        let agents = (episodes / 16).max(1);
        Shape {
            n,
            seed,
            nodes,
            episodes,
            agents,
        }
    }
    pub fn episode_of(&self, node: u64) -> u64 {
        node.wrapping_mul(2_654_435_761) % self.episodes
    }
    pub fn agent_of_episode(&self, e: u64) -> u64 {
        e % self.agents
    }
    pub fn episodes_of_agent(&self, a: u64) -> Vec<String> {
        (0..self.episodes)
            .filter(|e| e % self.agents == a)
            .map(episode_iri)
            .collect()
    }
}

/// A node-valued statement the harness can address again (compact form).
#[derive(Clone, Copy, Debug)]
pub struct Rec {
    pub native: Option<u64>,
    pub s: u64,
    pub p: u64,
    pub o: u64,
    pub ep: u64,
    pub live: bool,
}

impl Rec {
    pub fn to_ref(self) -> StmtRef {
        StmtRef {
            native: self.native,
            s: Term::Iri(node_iri(self.s)),
            p: pred(self.p),
            o: Term::Iri(node_iri(self.o)),
            g: Some(episode_iri(self.ep)),
        }
    }
}

/// One generated statement.
pub enum Gen {
    Edge { s: u64, p: u64, o: u64 },
    Prop { s: u64, p: u64, v: i64 },
}

/// The statement stream, subject by subject.
pub fn stream(shape: &Shape, mut f: impl FnMut(Gen)) {
    let mut rng = SmallRng::seed_from_u64(shape.seed);
    let mut k = 0u64;
    let mut edges: Vec<(u64, u64)> = Vec::with_capacity(16);
    for node in 0..shape.nodes {
        let deg = if node == shape.nodes - 1 {
            shape.n.saturating_sub(k)
        } else {
            rng.gen_range(4..=12u64).min(shape.n.saturating_sub(k))
        };
        edges.clear();
        for _ in 0..deg {
            edges.push((rand_pred(&mut rng), rand_obj(&mut rng, shape.nodes)));
        }
        edges.sort_unstable();
        edges.dedup();
        for &(p, o) in &edges {
            k += 1;
            f(Gen::Edge { s: node, p, o });
        }
        let mut lps: Vec<u64> = Vec::with_capacity(PROPS_PER_NODE);
        while lps.len() < PROPS_PER_NODE {
            let p = rng.gen_range(0..LPRED);
            if !lps.contains(&p) {
                lps.push(p);
            }
        }
        lps.sort_unstable();
        for p in lps {
            f(Gen::Prop {
                s: node,
                p,
                v: rng.gen_range(0..1_000_000),
            });
        }
    }
}

/// A loaded dataset: shape plus the node-valued statements with handles.
#[derive(Debug)]
pub struct Loaded {
    pub shape: Shape,
    pub recs: Vec<Rec>,
    pub load: Value,
}

/// Loads the dataset through the adapter in write batches of `batch`
/// statements at `relaxed`, then a checkpoint, so handles are captured.
pub fn load(a: &dyn Adapter, shape: &Shape, batch: usize) -> Result<Loaded> {
    let mut s = a.session()?;
    let t0 = Instant::now();
    // Graph structure first: episodes nested under agents.
    let nest: Vec<WOp> = (0..shape.episodes)
        .map(|e| WOp::Nest {
            child: episode_iri(e),
            parent: agent_iri(shape.agent_of_episode(e)),
        })
        .collect();
    for c in nest.chunks(batch) {
        s.write(c, Dur::Relaxed)?;
    }
    let mut recs: Vec<Rec> = Vec::with_capacity(shape.n as usize);
    let mut ops: Vec<WOp> = Vec::with_capacity(batch);
    let mut pending: Vec<Rec> = Vec::new();
    let mut nstmts = 0u64;
    let mut err = None;
    let mut flush = |ops: &mut Vec<WOp>, pending: &mut Vec<Rec>, recs: &mut Vec<Rec>| -> Result<()> {
        let ids = s.write(ops, Dur::Relaxed)?;
        // One id per Insert in order; edges and props are interleaved.
        let mut it = ids.into_iter();
        let mut pi = 0;
        for op in ops.iter() {
            if let WOp::Insert { o, .. } = op {
                let id = it.next().flatten();
                if matches!(o, Term::Iri(_)) {
                    let mut r = pending[pi];
                    r.native = id;
                    recs.push(r);
                    pi += 1;
                }
            }
        }
        ops.clear();
        pending.clear();
        Ok(())
    };
    stream(shape, |g| {
        if err.is_some() {
            return;
        }
        nstmts += 1;
        match g {
            Gen::Edge { s: sn, p, o } => {
                let ep = shape.episode_of(sn);
                ops.push(WOp::Insert {
                    s: Term::Iri(node_iri(sn)),
                    p: pred(p),
                    o: Term::Iri(node_iri(o)),
                    g: Some(episode_iri(ep)),
                });
                pending.push(Rec {
                    native: None,
                    s: sn,
                    p,
                    o,
                    ep,
                    live: true,
                });
            }
            Gen::Prop { s: sn, p, v } => ops.push(WOp::Insert {
                s: Term::Iri(node_iri(sn)),
                p: lpred(p),
                o: Term::int(v),
                g: Some(episode_iri(shape.episode_of(sn))),
            }),
        }
        if ops.len() >= batch {
            if let Err(e) = flush(&mut ops, &mut pending, &mut recs) {
                err = Some(e);
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    if !ops.is_empty() {
        flush(&mut ops, &mut pending, &mut recs)?;
    }
    drop(s);
    a.checkpoint()?;
    let secs = t0.elapsed().as_secs_f64();
    let size = a.disk_size();
    Ok(Loaded {
        shape: *shape,
        recs,
        load: json!({
            "statements": nstmts,
            "node_statements": shape.n,
            "load_s": secs,
            "statements_per_s": nstmts as f64 / secs.max(1e-9),
            "disk_bytes": size,
            "bytes_per_statement": size as f64 / nstmts.max(1) as f64,
            "batch": batch,
            "batch_durability": "relaxed, then one durable checkpoint",
        }),
    })
}

/// Writes the dataset as N-Quads (for loading into other tools).
pub fn write_nquads(shape: &Shape, w: &mut impl std::io::Write) -> Result<u64> {
    let mut n = 0u64;
    let mut res = Ok(());
    stream(shape, |g| {
        if res.is_err() {
            return;
        }
        let (s, p, o) = match g {
            Gen::Edge { s, p, o } => (s, pred(p), Term::Iri(node_iri(o))),
            Gen::Prop { s, p, v } => (s, lpred(p), Term::int(v)),
        };
        let g = episode_iri(shape.episode_of(s));
        res = writeln!(w, "<{}> {} {} <{g}> .", node_iri(s), p.nt(), o.nt());
        n += 1;
    });
    res?;
    for e in 0..shape.episodes {
        writeln!(
            w,
            "<{}> <https://graphlava.org/ns#in> <{}> .",
            episode_iri(e),
            agent_iri(shape.agent_of_episode(e))
        )?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let sh = Shape::new(10_000, 7);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        write_nquads(&sh, &mut a).unwrap();
        write_nquads(&sh, &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(node_index(&node_iri(42)), Some(42));
    }
}
