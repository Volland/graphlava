//! tiramemsu ([[17-benchmarks#Versus tiramemsu]]): the bitemporal triple store
//! on SQLite, behind cargo feature `tiramemsu` (git dependency, Rust 1.88+).
//!
//! Durability is set per database in tiramemsu, not per commit: the adapter
//! opens with `sync` -> `Immediate` (synchronous=FULL), `ordered` ->
//! `Immediate`, `relaxed` -> `Normal` (synchronous=NORMAL), and records a
//! mismatch when a workload asks for another level per write. Overwrites
//! retract the live `(s, p, *)` statements and assert the new value, so every
//! overwrite adds a version (tiramemsu never forgets). Scoped reads go
//! through graph membership (`graph_members`).

use super::{unsupported, Adapter, Dur, NodeH, OpenCfg, Quad, Scope, Session, StmtRef, Term, WOp};
use anyhow::{anyhow, Result};
use serde_json::{json, Value as J};
use tiramemsu::{AssertOpts, Db, Durability, Eid, ObjectId, OpenOptions, SparqlResult, TxOptions, Valid, Value};

pub struct TiramemsuAdapter {
    db: Db,
    dir: std::path::PathBuf,
    dur: Dur,
}

impl std::fmt::Debug for TiramemsuAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TiramemsuAdapter").field("dir", &self.dir).finish_non_exhaustive()
    }
}

fn val(t: &Term) -> Value {
    match t {
        Term::Iri(i) => Value::iri(i.clone()),
        Term::Blank(b) => Value::iri(format!("urn:glv-bench:blank:{b}")),
        Term::Lit { lex, dt, lang } => Value::literal(lex, if lang.is_some() { None } else { Some(dt) }, lang.as_deref()),
    }
}

impl TiramemsuAdapter {
    pub fn open(cfg: &OpenCfg) -> Result<Self> {
        let dir = cfg.dir.join("tiramemsu");
        std::fs::create_dir_all(&dir)?;
        let durability = match cfg.dur {
            Dur::Sync | Dur::Ordered => Durability::Immediate,
            Dur::Relaxed => Durability::Normal,
        };
        let db = Db::open(dir.join("memory.db"), OpenOptions { durability, ..OpenOptions::default() })
            .map_err(|e| anyhow!("{e}"))?;
        Ok(TiramemsuAdapter { db, dir, dur: cfg.dur })
    }
}

impl Adapter for TiramemsuAdapter {
    fn name(&self) -> &'static str {
        "tiramemsu"
    }
    fn describe(&self) -> J {
        json!({
            "engine": "tiramemsu",
            "source": "git https://github.com/Volland/tiramemsu (see bench/Cargo.lock for the commit)",
            "backend": "SQLite (default backend)",
            "opened_with": self.dur,
            "durability_mapping": {
                "sync": "Durability::Immediate (synchronous=FULL), per database",
                "ordered": "Durability::Immediate (no group commit), per database",
                "relaxed": "Durability::Normal (synchronous=NORMAL), per database",
            },
            "overwrite": "retract live (s,p,*) and assert: one new version per overwrite",
        })
    }
    fn session(&self) -> Result<Box<dyn Session + '_>> {
        Ok(Box::new(TmSession { db: &self.db }))
    }
    fn disk_size(&self) -> u64 {
        crate::env::path_size(&self.dir)
    }
    fn checkpoint(&self) -> Result<()> {
        self.db.sync().map_err(|e| anyhow!("{e}"))
    }
}

struct TmSession<'a> {
    db: &'a Db,
}

fn e(x: tiramemsu::Error) -> anyhow::Error {
    anyhow!("{x}")
}

impl Session for TmSession<'_> {
    fn node(&mut self, iri: &str) -> Result<Option<NodeH>> {
        Ok(self.db.now().encode(&Value::iri(iri)).map_err(e)?.map(|o| o.raw() as u64))
    }
    fn props(&mut self, n: NodeH) -> Result<usize> {
        let v = self.db.now();
        let rows = v.triples(Some(ObjectId::from_raw(n as i64)), None, None).map_err(e)?;
        Ok(rows
            .iter()
            .filter(|t| !matches!(v.decode(t.o), Ok(Value::Iri(_))))
            .count())
    }
    fn stmt(&mut self, st: &StmtRef) -> Result<bool> {
        // No public read-by-eid on View: look the triple up (SPO) and match the eid.
        let v = self.db.now();
        let (Some(s), Some(p), Some(o)) = (
            v.encode(&val(&st.s)).map_err(e)?,
            v.encode(&val(&st.p)).map_err(e)?,
            v.encode(&val(&st.o)).map_err(e)?,
        ) else {
            return Ok(false);
        };
        let rows = v.triples(Some(s), Some(p), Some(o)).map_err(e)?;
        Ok(match st.native {
            Some(n) => rows.iter().any(|t| t.eid.n() == n),
            None => !rows.is_empty(),
        })
    }
    fn out(&mut self, n: NodeH, limit: usize, scope: Option<&Scope>) -> Result<Vec<NodeH>> {
        let v = self.db.now();
        let mut rows = v.triples(Some(ObjectId::from_raw(n as i64)), None, None).map_err(e)?;
        if let Some(sc) = scope {
            let mut members = std::collections::HashSet::new();
            for g in &sc.episodes {
                if let Some(gid) = v.encode(&Value::iri(g.clone())).map_err(e)? {
                    members.extend(v.graph_members(gid).map_err(e)?);
                }
            }
            rows.retain(|t| members.contains(&t.eid));
        }
        Ok(rows
            .into_iter()
            .filter(|t| matches!(v.decode(t.o), Ok(Value::Iri(_))))
            .take(limit)
            .map(|t| t.o.raw() as u64)
            .collect())
    }
    fn star(&mut self, n: NodeH, preds: &[Term], scope: Option<&Scope>) -> Result<usize> {
        if scope.is_some() {
            return unsupported("scoped star on tiramemsu (graph filter per predicate)");
        }
        let v = self.db.now();
        let mut k = 0;
        for p in preds {
            if let Some(pid) = v.encode(&val(p)).map_err(e)? {
                k += v.triples(Some(ObjectId::from_raw(n as i64)), Some(pid), None).map_err(e)?.len();
            }
        }
        Ok(k)
    }
    fn write(&mut self, ops: &[WOp], _d: Dur) -> Result<Vec<Option<u64>>> {
        let mut out = Vec::new();
        self.db
            .transact(TxOptions::default(), |tx| {
                for op in ops {
                    match op {
                        WOp::Insert { s, p, o, g } => {
                            let eid = tx.assert(val(s), val(p), val(o), Valid::ALWAYS)?.eid();
                            if let Some(g) = g {
                                tx.add_to_graph(eid, Value::iri(g.clone()), AssertOpts::default())?;
                            }
                            out.push(Some(eid.n()));
                        }
                        WOp::Overwrite { s, p, o } => {
                            let (si, pi) = (tx.encode(val(s))?, tx.encode(val(p))?);
                            tx.retract_matching(Some(si), Some(pi), None)?;
                            tx.assert(val(s), val(p), val(o), Valid::ALWAYS)?;
                        }
                        WOp::Delete(st) => {
                            if let Some(n) = st.native {
                                tx.retract(Eid::new(n))?;
                            }
                        }
                        WOp::Annotate { target, p, o } => {
                            if let Some(n) = target.native {
                                tx.assert(Value::Stmt(Eid::new(n)), val(p), val(o), Valid::ALWAYS)?;
                            }
                        }
                        WOp::AddMember { target, g } => {
                            if let Some(n) = target.native {
                                tx.add_to_graph(Eid::new(n), Value::iri(g.clone()), AssertOpts::default())?;
                            }
                        }
                        WOp::Nest { .. } => {} // no graph nesting; scopes list their episodes
                    }
                }
                Ok(())
            })
            .map_err(e)?;
        Ok(out)
    }
    fn load(&mut self, quads: &[Quad]) -> Result<()> {
        for chunk in quads.chunks(20_000) {
            let ops: Vec<WOp> = chunk
                .iter()
                .map(|q| WOp::Insert { s: q.s.clone(), p: q.p.clone(), o: q.o.clone(), g: q.g.clone() })
                .collect();
            self.write(&ops, Dur::Relaxed)?;
        }
        Ok(())
    }
    fn sparql(&mut self, q: &str) -> Result<usize> {
        Ok(match self.db.now().sparql(q).map_err(e)? {
            SparqlResult::Solutions(s) => s.rows.len(),
            SparqlResult::Boolean(b) => usize::from(b),
            SparqlResult::Graph(g) => g.len(),
            SparqlResult::Update(_) => 0,
        })
    }
    fn sparql_update(&mut self, u: &str, _d: Dur) -> Result<()> {
        self.db.now().sparql(u).map_err(e)?;
        Ok(())
    }
}
