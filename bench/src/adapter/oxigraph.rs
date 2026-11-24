//! Oxigraph 0.4.11, the closest embedded Rust RDF store
//! ([[17-benchmarks#Standard SPARQL workloads]]).
//!
//! Storage: RocksDB on disk (feature `oxigraph-rocksdb`, on by default); built
//! without it, the in-memory store. Durability: Oxigraph exposes no sync
//! option; each transaction writes the RocksDB WAL without fsync, so every
//! level runs as "process-crash safe, not power-loss safe" (closest to
//! `relaxed`) and results at `sync`/`ordered` carry `durability_matches: false`.
//! Version note: 0.5.x pins `spargebra =0.4.8` with feature `sep-0002`, which
//! feature-unifies into graphlava's spargebra and breaks graphlava's build.
//!
//! Model: episode graphs are named graphs; scoped reads union the scope's
//! episodes (Oxigraph has no graph nesting); statements about statements use
//! an RDF-star quoted triple as subject.

use super::{Adapter, Dur, NodeH, OpenCfg, Quad, Scope, Session, StmtRef, Term, WOp};
use crate::gen::{node_index, node_iri};
use anyhow::{anyhow, Result};
use oxigraph::model as om;
use oxigraph::sparql::QueryResults;
use oxigraph::store::{StorageError, Store};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct OxigraphAdapter {
    store: Store,
    dir: PathBuf,
    on_disk: bool,
}

pub fn nn(i: &str) -> om::NamedNode {
    om::NamedNode::new_unchecked(i)
}

pub fn term(t: &Term) -> om::Term {
    match t {
        Term::Iri(i) => nn(i).into(),
        Term::Blank(b) => om::BlankNode::new_unchecked(b.clone()).into(),
        Term::Lit { lex, dt, lang } => match lang {
            Some(l) => om::Literal::new_language_tagged_literal_unchecked(lex.clone(), l.clone()).into(),
            None => om::Literal::new_typed_literal(lex.clone(), nn(dt)).into(),
        },
    }
}

fn subject(t: &Term) -> Result<om::Subject> {
    Ok(match t {
        Term::Iri(i) => nn(i).into(),
        Term::Blank(b) => om::BlankNode::new_unchecked(b.clone()).into(),
        Term::Lit { .. } => return Err(anyhow!("literal subject")),
    })
}

fn graph(g: Option<&str>) -> om::GraphName {
    match g {
        Some(g) => nn(g).into(),
        None => om::GraphName::DefaultGraph,
    }
}

fn quoted(st: &StmtRef) -> Result<om::Subject> {
    Ok(om::Subject::Triple(Box::new(om::Triple::new(
        subject(&st.s)?,
        nn(match &st.p {
            Term::Iri(i) => i,
            _ => return Err(anyhow!("non-IRI predicate")),
        }),
        term(&st.o),
    ))))
}

fn pred(t: &Term) -> Result<om::NamedNode> {
    match t {
        Term::Iri(i) => Ok(nn(i)),
        _ => Err(anyhow!("non-IRI predicate")),
    }
}

pub fn quad(q: &Quad) -> Result<om::Quad> {
    Ok(om::Quad::new(subject(&q.s)?, pred(&q.p)?, term(&q.o), graph(q.g.as_deref())))
}

impl std::fmt::Debug for OxigraphAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OxigraphAdapter").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl OxigraphAdapter {
    pub fn open(cfg: &OpenCfg) -> Result<Self> {
        let dir = cfg.dir.join("oxigraph");
        #[cfg(feature = "oxigraph-rocksdb")]
        let (store, on_disk) = (Store::open(&dir)?, true);
        #[cfg(not(feature = "oxigraph-rocksdb"))]
        let (store, on_disk) = (Store::new()?, false);
        Ok(OxigraphAdapter { store, dir, on_disk })
    }
}

impl Adapter for OxigraphAdapter {
    fn name(&self) -> &'static str {
        "oxigraph"
    }
    fn describe(&self) -> Value {
        json!({
            "engine": "oxigraph",
            "version": "0.4.11",
            "storage": if self.on_disk { "RocksDB (oxrocksdb-sys, bundled), directory" } else { "in-memory (built without oxigraph-rocksdb)" },
            "durability_mapping": {
                "sync": "no per-commit fsync available: RocksDB WAL write, no sync (durability_matches=false)",
                "ordered": "same as sync (durability_matches=false)",
                "relaxed": "RocksDB WAL write, no sync",
            },
            "bulk_load": "Store::bulk_loader (standard workloads)",
            "graph_nesting": "none: scoped reads union the scope's episode graphs",
            "annotations": "RDF-star quoted-triple subject",
        })
    }
    fn session(&self) -> Result<Box<dyn Session + '_>> {
        Ok(Box::new(OxSession { st: &self.store }))
    }
    fn disk_size(&self) -> u64 {
        crate::env::path_size(&self.dir)
    }
    fn checkpoint(&self) -> Result<()> {
        if self.on_disk {
            self.store.flush()?;
        }
        Ok(())
    }
}

struct OxSession<'a> {
    st: &'a Store,
}

fn count_results(r: QueryResults) -> Result<usize> {
    Ok(match r {
        QueryResults::Solutions(it) => {
            let mut n = 0;
            for s in it {
                s?;
                n += 1;
            }
            n
        }
        QueryResults::Boolean(b) => usize::from(b),
        QueryResults::Graph(it) => {
            let mut n = 0;
            for t in it {
                t?;
                n += 1;
            }
            n
        }
    })
}

impl Session for OxSession<'_> {
    fn node(&mut self, iri: &str) -> Result<Option<NodeH>> {
        let Some(i) = node_index(iri) else { return Ok(None) };
        let s = nn(iri);
        let exists = self
            .st
            .quads_for_pattern(Some(s.as_ref().into()), None, None, None)
            .next()
            .transpose()?
            .is_some();
        Ok(exists.then_some(i))
    }
    fn props(&mut self, n: NodeH) -> Result<usize> {
        let s = nn(&node_iri(n));
        let mut k = 0;
        for q in self.st.quads_for_pattern(Some(s.as_ref().into()), None, None, None) {
            if matches!(q?.object, om::Term::Literal(_)) {
                k += 1;
            }
        }
        Ok(k)
    }
    fn stmt(&mut self, st: &StmtRef) -> Result<bool> {
        let s = subject(&st.s)?;
        let p = pred(&st.p)?;
        let o = term(&st.o);
        Ok(self
            .st
            .quads_for_pattern(Some(s.as_ref()), Some(p.as_ref()), Some(o.as_ref()), None)
            .next()
            .transpose()?
            .is_some())
    }
    fn out(&mut self, n: NodeH, limit: usize, scope: Option<&Scope>) -> Result<Vec<NodeH>> {
        let s = nn(&node_iri(n));
        let mut v = Vec::new();
        let graphs: Vec<Option<om::NamedNode>> = match scope {
            None => vec![None],
            Some(sc) => sc.episodes.iter().map(|e| Some(nn(e))).collect(),
        };
        'g: for g in &graphs {
            let gr = g.as_ref().map(|g| om::GraphNameRef::from(g.as_ref()));
            for q in self.st.quads_for_pattern(Some(s.as_ref().into()), None, None, gr) {
                if let om::Term::NamedNode(o) = q?.object {
                    if let Some(i) = node_index(o.as_str()) {
                        if !v.contains(&i) {
                            v.push(i);
                            if v.len() >= limit {
                                break 'g;
                            }
                        }
                    }
                }
            }
        }
        Ok(v)
    }
    fn star(&mut self, n: NodeH, preds: &[Term], scope: Option<&Scope>) -> Result<usize> {
        let s = nn(&node_iri(n));
        let mut k = 0;
        let graphs: Vec<Option<om::NamedNode>> = match scope {
            None => vec![None],
            Some(sc) => sc.episodes.iter().map(|e| Some(nn(e))).collect(),
        };
        for p in preds {
            let p = pred(p)?;
            for g in &graphs {
                let gr = g.as_ref().map(|g| om::GraphNameRef::from(g.as_ref()));
                for q in self
                    .st
                    .quads_for_pattern(Some(s.as_ref().into()), Some(p.as_ref()), None, gr)
                {
                    q?;
                    k += 1;
                }
            }
        }
        Ok(k)
    }
    fn write(&mut self, ops: &[WOp], _d: Dur) -> Result<Vec<Option<u64>>> {
        let ninserts = ops.iter().filter(|o| matches!(o, WOp::Insert { .. })).count();
        // Convert outside the (retryable) transaction closure.
        let st = self.st;
        st.transaction(|mut t| -> Result<(), StorageError> {
            for op in ops {
                match op {
                    WOp::Insert { s, p, o, g } => {
                        let (Ok(s), Ok(p)) = (subject(s), pred(p)) else { continue };
                        t.insert(om::QuadRef::new(&s, &p, &term(o), &graph(g.as_deref())))?;
                    }
                    WOp::Overwrite { s, p, o } => {
                        let (Ok(s), Ok(p)) = (subject(s), pred(p)) else { continue };
                        let old: Vec<om::Quad> = t
                            .quads_for_pattern(Some(s.as_ref()), Some(p.as_ref()), None, None)
                            .collect::<Result<_, _>>()?;
                        let g = old.first().map(|q| q.graph_name.clone()).unwrap_or_default();
                        for q in &old {
                            t.remove(q)?;
                        }
                        t.insert(om::QuadRef::new(&s, &p, &term(o), &g))?;
                    }
                    WOp::Delete(sr) => {
                        let (Ok(s), Ok(p)) = (subject(&sr.s), pred(&sr.p)) else { continue };
                        let o = term(&sr.o);
                        let old: Vec<om::Quad> = t
                            .quads_for_pattern(Some(s.as_ref()), Some(p.as_ref()), Some(o.as_ref()), None)
                            .collect::<Result<_, _>>()?;
                        for q in &old {
                            t.remove(q)?;
                        }
                        if let Ok(qs) = quoted(sr) {
                            let ann: Vec<om::Quad> = t
                                .quads_for_pattern(Some(qs.as_ref()), None, None, None)
                                .collect::<Result<_, _>>()?;
                            for q in &ann {
                                t.remove(q)?;
                            }
                        }
                    }
                    WOp::Annotate { target, p, o } => {
                        let (Ok(qs), Ok(p)) = (quoted(target), pred(p)) else { continue };
                        t.insert(om::QuadRef::new(&qs, &p, &term(o), om::GraphNameRef::DefaultGraph))?;
                    }
                    WOp::AddMember { target, g } => {
                        let (Ok(s), Ok(p)) = (subject(&target.s), pred(&target.p)) else { continue };
                        t.insert(om::QuadRef::new(&s, &p, &term(&target.o), &graph(Some(g))))?;
                    }
                    WOp::Nest { child, parent } => {
                        let p = nn("https://graphlava.org/ns#in");
                        t.insert(om::QuadRef::new(&nn(child), &p, &nn(parent), om::GraphNameRef::DefaultGraph))?;
                    }
                }
            }
            Ok(())
        })?;
        Ok(vec![None; ninserts])
    }
    fn load(&mut self, quads: &[Quad]) -> Result<()> {
        let qs: Vec<om::Quad> = quads.iter().filter_map(|q| quad(q).ok()).collect();
        self.st.bulk_loader().load_quads(qs)?;
        Ok(())
    }
    fn sparql(&mut self, q: &str) -> Result<usize> {
        count_results(self.st.query(q)?)
    }
    fn sparql_update(&mut self, u: &str, _d: Dur) -> Result<()> {
        self.st.update(u)?;
        Ok(())
    }
    fn range_gt(&mut self, p: &Term, lo: &Term) -> Result<usize> {
        let q = format!("SELECT ?s WHERE {{ ?s {} ?v FILTER(?v > {}) }}", p.nt(), lo.nt());
        self.sparql(&q)
    }
}
