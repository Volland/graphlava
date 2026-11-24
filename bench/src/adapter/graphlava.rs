//! graphlava through its public API: one-shot `submit` requests for writes
//! (the group-commit path), snapshot `Reader` calls for the primitives,
//! `sparql`/`datalog` text for the standard workloads.

use super::{Adapter, Dur, NodeH, OpenCfg, Quad, Scope, Session, StmtRef, Term, WOp};
use anyhow::{anyhow, Result};
use graphlava::id::Kind;
use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Id, Literal, OpResult, OpenOptions, QueryResult,
    Reader, WriteRequest,
};
use serde_json::{json, Value};
use std::ops::Bound;
use std::path::PathBuf;
use std::time::Duration;

pub fn dur(d: Dur) -> Durability {
    match d {
        Dur::Sync => Durability::Sync,
        Dur::Ordered => Durability::Ordered,
        Dur::Relaxed => Durability::Relaxed,
    }
}

pub fn term(t: &Term) -> graphlava::Term {
    match t {
        Term::Iri(i) => graphlava::Term::iri(i.clone()),
        Term::Blank(b) => graphlava::Term::Blank(b.clone()),
        Term::Lit { lex, dt, lang } => match lang {
            Some(l) => graphlava::Term::lit(Literal::lang(lex.clone(), l.clone())),
            None => graphlava::Term::lit(Literal::typed(lex.clone(), dt.clone())),
        },
    }
}

#[derive(Debug)]
pub struct GraphlavaAdapter {
    pub db: Option<Database>,
    pub path: PathBuf,
    pub opts: OpenOptions,
}

impl GraphlavaAdapter {
    pub fn options(cfg: &OpenCfg) -> OpenOptions {
        OpenOptions {
            group_window: Duration::from_millis(cfg.group_window_ms),
            cache_bytes: cfg.cache_bytes,
            ..OpenOptions::default()
        }
    }
    pub fn open(cfg: &OpenCfg) -> Result<Self> {
        let path = cfg.dir.join("graphlava.glv");
        let opts = Self::options(cfg);
        let db = Database::open(&path, opts.clone())?;
        Ok(GraphlavaAdapter {
            db: Some(db),
            path,
            opts,
        })
    }
    pub fn db(&self) -> &Database {
        self.db.as_ref().expect("open database")
    }
}

impl Drop for GraphlavaAdapter {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            let _ = db.close();
        }
    }
}

impl Adapter for GraphlavaAdapter {
    fn name(&self) -> &'static str {
        "graphlava"
    }
    fn describe(&self) -> Value {
        json!({
            "engine": "graphlava",
            "file": self.path.display().to_string(),
            "version": graphlava::VERSION,
            "storage": "redb 2.6.4, single file",
            "group_window_ms": self.opts.group_window.as_millis() as u64,
            "cache_bytes": self.opts.cache_bytes,
            "max_relaxed_run": self.opts.max_relaxed_run,
            "durability_mapping": {
                "sync": "Durability::Sync (durable on return, closes the batch)",
                "ordered": "Durability::Ordered (durable on return, group commit)",
                "relaxed": "Durability::Relaxed (atomic, durable at a later durable commit)",
            },
            "write_path": "Database::submit (one-shot request), Reader for point reads",
        })
    }
    fn session(&self) -> Result<Box<dyn Session + '_>> {
        Ok(Box::new(GlvSession { db: self.db() }))
    }
    fn disk_size(&self) -> u64 {
        self.db().file_size().unwrap_or(0)
    }
    fn checkpoint(&self) -> Result<()> {
        self.db()
            .submit(WriteRequest::default(), Durability::Sync)?;
        Ok(())
    }
    fn as_graphlava(&self) -> Option<&GraphlavaAdapter> {
        Some(self)
    }
}

struct GlvSession<'a> {
    db: &'a Database,
}

fn is_node(id: Id) -> bool {
    matches!(id.kind(), Kind::Iri | Kind::Blank | Kind::Statement | Kind::Hyperedge)
}

fn rows(r: &QueryResult) -> usize {
    match r {
        QueryResult::Solutions { rows, .. } => rows.len(),
        QueryResult::Boolean(b) => usize::from(*b),
        QueryResult::Graph(g) => g.len(),
    }
}

impl GlvSession<'_> {
    fn id_of(&self, r: &impl Reader, t: &Term) -> Result<Option<Id>> {
        Ok(r.lookup(&term(t))?)
    }
}

impl Session for GlvSession<'_> {
    fn node(&mut self, iri: &str) -> Result<Option<NodeH>> {
        let r = self.db.read()?;
        Ok(r.lookup(&graphlava::Term::iri(iri))?.map(|i| i.raw()))
    }
    fn props(&mut self, n: NodeH) -> Result<usize> {
        Ok(self.db.read()?.props(Id(n))?.len())
    }
    fn stmt(&mut self, st: &StmtRef) -> Result<bool> {
        let r = self.db.read()?;
        if let Some(id) = st.native {
            return Ok(r.stmt(Id(id))?.is_some());
        }
        let (Some(s), Some(p), Some(o)) = (
            self.id_of(&r, &st.s)?,
            self.id_of(&r, &st.p)?,
            self.id_of(&r, &st.o)?,
        ) else {
            return Ok(false);
        };
        Ok(!r.find(Some(s), Some(p), Some(o))?.is_empty())
    }
    fn out(&mut self, n: NodeH, limit: usize, scope: Option<&Scope>) -> Result<Vec<NodeH>> {
        let r = self.db.read()?;
        let ms = match scope {
            None => r.out(Id(n), None)?,
            Some(sc) => match r.graph(&graphlava::Term::iri(sc.graph.clone()))? {
                Some(g) => r.find_in(g, false, Some(Id(n)), None, None)?,
                None => Vec::new(),
            },
        };
        Ok(ms
            .into_iter()
            .filter(|m| is_node(m.o))
            .take(limit)
            .map(|m| m.o.raw())
            .collect())
    }
    fn star(&mut self, n: NodeH, preds: &[Term], scope: Option<&Scope>) -> Result<usize> {
        let r = self.db.read()?;
        let g = match scope {
            Some(sc) => match r.graph(&graphlava::Term::iri(sc.graph.clone()))? {
                Some(g) => Some(g),
                None => return Ok(0),
            },
            None => None,
        };
        let mut rows = 0;
        for p in preds {
            let Some(pid) = self.id_of(&r, p)? else { continue };
            rows += match g {
                None => r.out(Id(n), Some(pid))?.len(),
                Some(g) => r.find_in(g, false, Some(Id(n)), Some(pid), None)?.len(),
            };
        }
        Ok(rows)
    }
    fn write(&mut self, ops: &[WOp], d: Dur) -> Result<Vec<Option<u64>>> {
        let mut req = WriteRequest::default();
        let mut inserts = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            req.ops.push(match op {
                WOp::Insert { s, p, o, g } => {
                    inserts.push(i);
                    CoreOp::Insert {
                        s: term(s).into(),
                        p: term(p).into(),
                        o: term(o).into(),
                        graph: g.as_ref().map(|g| graphlava::Term::iri(g.clone()).into()),
                    }
                }
                WOp::Overwrite { s, p, o } => CoreOp::Set {
                    s: term(s).into(),
                    p: term(p).into(),
                    o: term(o).into(),
                },
                WOp::Delete(st) => match st.native {
                    Some(id) => CoreOp::Delete {
                        id: Id(id),
                        mode: DeleteMode::Cascade,
                    },
                    None => CoreOp::Remove {
                        s: term(&st.s).into(),
                        p: term(&st.p).into(),
                        o: term(&st.o).into(),
                        mode: DeleteMode::Cascade,
                        graph: None,
                    },
                },
                WOp::Annotate { target, p, o } => CoreOp::Annotate {
                    id: Id(target
                        .native
                        .ok_or_else(|| anyhow!("annotation target without a statement ID"))?),
                    p: term(p).into(),
                    o: term(o).into(),
                },
                WOp::AddMember { target, g } => CoreOp::AddMember {
                    item: Id(target
                        .native
                        .ok_or_else(|| anyhow!("membership target without a statement ID"))?),
                    graph: graphlava::Term::iri(g.clone()).into(),
                },
                WOp::Nest { child, parent } => CoreOp::Nest {
                    child: graphlava::Term::iri(child.clone()).into(),
                    parent: graphlava::Term::iri(parent.clone()).into(),
                },
            });
        }
        let t = self.db.submit(req, dur(d))?;
        Ok(inserts
            .into_iter()
            .map(|i| match &t.results[i] {
                OpResult::Id(id) => id.map(|x| x.raw()),
                _ => None,
            })
            .collect())
    }
    fn load(&mut self, quads: &[Quad]) -> Result<()> {
        for chunk in quads.chunks(20_000) {
            let ops = chunk
                .iter()
                .map(|q| CoreOp::Insert {
                    s: term(&q.s).into(),
                    p: term(&q.p).into(),
                    o: term(&q.o).into(),
                    graph: q.g.as_ref().map(|g| graphlava::Term::iri(g.clone()).into()),
                })
                .collect();
            self.db.submit(WriteRequest { ops }, Durability::Relaxed)?;
        }
        Ok(())
    }
    fn sparql(&mut self, q: &str) -> Result<usize> {
        Ok(rows(&self.db.sparql(q)?))
    }
    fn sparql_update(&mut self, u: &str, d: Dur) -> Result<()> {
        self.db.sparql_update(u, dur(d))?;
        Ok(())
    }
    fn datalog(&mut self, program: &str) -> Result<usize> {
        Ok(rows(&self.db.datalog(program)?))
    }
    fn range_gt(&mut self, p: &Term, lo: &Term) -> Result<usize> {
        let r = self.db.read()?;
        let (Some(pid), Some(lid)) = (self.id_of(&r, p)?, r.lookup(&term(lo))?) else {
            return Ok(0);
        };
        Ok(r.find_range(pid, Bound::Excluded(lid), Bound::Unbounded)?.0.len())
    }
}

/// Converts a graphlava term back (diagnostics); new variants are reported.
#[allow(dead_code)]
pub fn from_glv(t: &graphlava::Term) -> Result<Term> {
    Ok(match t {
        graphlava::Term::Iri(i) => Term::Iri(i.clone()),
        graphlava::Term::Blank(b) => Term::Blank(b.clone()),
        graphlava::Term::Literal(l) => Term::Lit {
            lex: l.lexical.clone(),
            dt: l.datatype.clone(),
            lang: l.lang.clone(),
        },
        // `graphlava::Term` is non_exhaustive (RDF 1.2 triple terms are coming).
        #[allow(unreachable_patterns)]
        other => return Err(anyhow!("unsupported graphlava term: {other:?}")),
    })
}
