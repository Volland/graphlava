//! SQLite baseline ([[17-benchmarks#Versus SQLite and Turso]]): a triple table
//! in WAL mode with the same three orderings as indexes.
//!
//! Schema (one file, `sqlite.db`):
//!
//! ```sql
//! term(id INTEGER PRIMARY KEY, nt TEXT UNIQUE, lit INTEGER, num REAL)  -- dictionary, N-Triples key
//!   + index term_num(num) WHERE num IS NOT NULL                      -- numeric range scans
//! stmt(id INTEGER PRIMARY KEY, s, p, o, olit)                         -- rowid = statement ID
//!   + indexes spo(s,p,o), pos(p,o,s), osp(o,s,p)                       -- the three orderings
//! member(g, stmt) PRIMARY KEY (g, stmt) WITHOUT ROWID + index (stmt)  -- named-graph membership
//! nest(child, parent) PRIMARY KEY WITHOUT ROWID                       -- graph nesting
//! ```
//!
//! Statements about statements use the subject term `<urn:glv-bench:stmt:ID>`.
//! A scoped read expands nesting with a recursive CTE. Overwrites are
//! `UPDATE stmt SET o = ? WHERE s = ? AND p = ?` (insert when absent).
//! Durability: `sync` and `ordered` -> `synchronous = FULL`; `relaxed` ->
//! `synchronous = NORMAL` (WAL: crash-consistent, may lose the newest commits).

use super::{unsupported, Adapter, Dur, NodeH, OpenCfg, Quad, Scope, Session, StmtRef, Term, WOp};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::PathBuf;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS term(id INTEGER PRIMARY KEY, nt TEXT NOT NULL UNIQUE, lit INTEGER NOT NULL, num REAL);
CREATE INDEX IF NOT EXISTS term_num ON term(num) WHERE num IS NOT NULL;
CREATE TABLE IF NOT EXISTS stmt(id INTEGER PRIMARY KEY, s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL, olit INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS spo ON stmt(s, p, o);
CREATE INDEX IF NOT EXISTS pos ON stmt(p, o, s);
CREATE INDEX IF NOT EXISTS osp ON stmt(o, s, p);
CREATE TABLE IF NOT EXISTS member(g INTEGER NOT NULL, stmt INTEGER NOT NULL, PRIMARY KEY(g, stmt)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS member_stmt ON member(stmt);
CREATE TABLE IF NOT EXISTS nest(child INTEGER NOT NULL, parent INTEGER NOT NULL, PRIMARY KEY(child, parent)) WITHOUT ROWID;
"#;

const SCOPE_CTE: &str = "WITH RECURSIVE sc(g) AS (SELECT id FROM term WHERE nt = ?2 UNION SELECT n.child FROM nest n JOIN sc ON n.parent = sc.g)";

#[derive(Debug)]
pub struct SqliteAdapter {
    path: PathBuf,
    dur: Dur,
}

fn sync_pragma(d: Dur) -> &'static str {
    match d {
        Dur::Sync | Dur::Ordered => "FULL",
        Dur::Relaxed => "NORMAL",
    }
}

fn connect(path: &PathBuf, d: Dur) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(30))?;
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.pragma_update(None, "synchronous", sync_pragma(d))?;
    c.pragma_update(None, "cache_size", -65536)?; // 64 MiB, as graphlava's default cache
    c.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(c)
}

impl SqliteAdapter {
    pub fn open(cfg: &OpenCfg) -> Result<Self> {
        let path = cfg.dir.join("sqlite.db");
        let c = connect(&path, cfg.dur)?;
        c.execute_batch(SCHEMA)?;
        Ok(SqliteAdapter { path, dur: cfg.dur })
    }
}

impl Adapter for SqliteAdapter {
    fn name(&self) -> &'static str {
        "sqlite"
    }
    fn describe(&self) -> Value {
        json!({
            "engine": "sqlite",
            "version": rusqlite::version(),
            "binding": "rusqlite 0.40 (bundled)",
            "journal_mode": "WAL",
            "cache_size_kib": 65536,
            "schema": SCHEMA.trim(),
            "durability_mapping": {
                "sync": "synchronous=FULL",
                "ordered": "synchronous=FULL (no group commit in SQLite)",
                "relaxed": "synchronous=NORMAL",
            },
            "sparql": "not supported (standard SPARQL suites skip SQLite)",
        })
    }
    fn session(&self) -> Result<Box<dyn Session + '_>> {
        Ok(Box::new(SqlSession {
            c: connect(&self.path, self.dur)?,
            cur: self.dur,
        }))
    }
    fn disk_size(&self) -> u64 {
        let mut n = 0;
        for ext in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{ext}", self.path.display()));
            n += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        }
        n
    }
    fn checkpoint(&self) -> Result<()> {
        let c = connect(&self.path, Dur::Sync)?;
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        c.execute_batch("ANALYZE")?;
        Ok(())
    }
}

struct SqlSession {
    c: Connection,
    cur: Dur,
}

fn stmt_term(id: u64) -> String {
    format!("<urn:glv-bench:stmt:{id}>")
}

impl SqlSession {
    fn set_dur(&mut self, d: Dur) -> Result<()> {
        if sync_pragma(d) != sync_pragma(self.cur) {
            self.c.pragma_update(None, "synchronous", sync_pragma(d))?;
        }
        self.cur = d;
        Ok(())
    }
    fn lookup(&self, nt: &str) -> Result<Option<i64>> {
        Ok(self
            .c
            .prepare_cached("SELECT id FROM term WHERE nt = ?1")?
            .query_row([nt], |r| r.get(0))
            .optional()?)
    }
}

fn intern(c: &Connection, t: &Term) -> Result<i64> {
    let nt = t.nt();
    if let Some(id) = c
        .prepare_cached("SELECT id FROM term WHERE nt = ?1")?
        .query_row([&nt], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    c.prepare_cached("INSERT INTO term(nt, lit, num) VALUES (?1, ?2, ?3)")?
        .execute(params![nt, matches!(t, Term::Lit { .. }) as i64, t.num()])?;
    Ok(c.last_insert_rowid())
}

fn intern_nt(c: &Connection, nt: &str) -> Result<i64> {
    if let Some(id) = c
        .prepare_cached("SELECT id FROM term WHERE nt = ?1")?
        .query_row([nt], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    c.prepare_cached("INSERT INTO term(nt, lit, num) VALUES (?1, 0, NULL)")?
        .execute([nt])?;
    Ok(c.last_insert_rowid())
}

fn insert_stmt(c: &Connection, s: &Term, p: &Term, o: &Term, g: Option<&str>) -> Result<i64> {
    let (s, p, oi) = (intern(c, s)?, intern(c, p)?, intern(c, o)?);
    let olit = matches!(o, Term::Lit { .. }) as i64;
    let existing: Option<i64> = c
        .prepare_cached("SELECT id FROM stmt WHERE s = ?1 AND p = ?2 AND o = ?3")?
        .query_row(params![s, p, oi], |r| r.get(0))
        .optional()?;
    let id = match existing {
        Some(id) => id,
        None => {
            c.prepare_cached("INSERT INTO stmt(s, p, o, olit) VALUES (?1, ?2, ?3, ?4)")?
                .execute(params![s, p, oi, olit])?;
            c.last_insert_rowid()
        }
    };
    if let Some(g) = g {
        let gid = intern_nt(c, &format!("<{g}>"))?;
        c.prepare_cached("INSERT OR IGNORE INTO member(g, stmt) VALUES (?1, ?2)")?
            .execute(params![gid, id])?;
    }
    Ok(id)
}

fn stmt_id_of(c: &Connection, st: &StmtRef) -> Result<Option<i64>> {
    if let Some(n) = st.native {
        return Ok(Some(n as i64));
    }
    let q = "SELECT st.id FROM stmt st JOIN term a ON a.id = st.s JOIN term b ON b.id = st.p JOIN term d ON d.id = st.o WHERE a.nt = ?1 AND b.nt = ?2 AND d.nt = ?3";
    Ok(c.prepare_cached(q)?
        .query_row(params![st.s.nt(), st.p.nt(), st.o.nt()], |r| r.get(0))
        .optional()?)
}

impl Session for SqlSession {
    fn node(&mut self, iri: &str) -> Result<Option<NodeH>> {
        Ok(self.lookup(&format!("<{iri}>"))?.map(|i| i as u64))
    }
    fn props(&mut self, n: NodeH) -> Result<usize> {
        let mut st = self
            .c
            .prepare_cached("SELECT p, o FROM stmt WHERE s = ?1 AND olit = 1")?;
        let mut rows = st.query([n as i64])?;
        let mut k = 0;
        while rows.next()?.is_some() {
            k += 1;
        }
        Ok(k)
    }
    fn stmt(&mut self, st: &StmtRef) -> Result<bool> {
        let Some(id) = stmt_id_of(&self.c, st)? else {
            return Ok(false);
        };
        Ok(self
            .c
            .prepare_cached("SELECT s, p, o FROM stmt WHERE id = ?1")?
            .query_row([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
            .optional()?
            .is_some())
    }
    fn out(&mut self, n: NodeH, limit: usize, scope: Option<&Scope>) -> Result<Vec<NodeH>> {
        let mut v = Vec::new();
        match scope {
            None => {
                let mut st = self.c.prepare_cached(
                    "SELECT o FROM stmt WHERE s = ?1 AND olit = 0 LIMIT ?2",
                )?;
                let mut rows = st.query(params![n as i64, limit as i64])?;
                while let Some(r) = rows.next()? {
                    v.push(r.get::<_, i64>(0)? as u64);
                }
            }
            Some(sc) => {
                let q = format!("{SCOPE_CTE} SELECT DISTINCT st.o FROM stmt st JOIN member m ON m.stmt = st.id WHERE st.s = ?1 AND st.olit = 0 AND m.g IN (SELECT g FROM sc) LIMIT ?3");
                let mut st = self.c.prepare_cached(&q)?;
                let mut rows = st.query(params![n as i64, format!("<{}>", sc.graph), limit as i64])?;
                while let Some(r) = rows.next()? {
                    v.push(r.get::<_, i64>(0)? as u64);
                }
            }
        }
        Ok(v)
    }
    fn star(&mut self, n: NodeH, preds: &[Term], scope: Option<&Scope>) -> Result<usize> {
        let mut k = 0;
        for p in preds {
            let Some(pid) = self.lookup(&p.nt())? else { continue };
            match scope {
                None => {
                    let mut st = self
                        .c
                        .prepare_cached("SELECT o FROM stmt WHERE s = ?1 AND p = ?2")?;
                    let mut rows = st.query(params![n as i64, pid])?;
                    while rows.next()?.is_some() {
                        k += 1;
                    }
                }
                Some(sc) => {
                    let q = format!("{SCOPE_CTE} SELECT DISTINCT st.id FROM stmt st JOIN member m ON m.stmt = st.id WHERE st.s = ?1 AND st.p = ?3 AND m.g IN (SELECT g FROM sc)");
                    let mut st = self.c.prepare_cached(&q)?;
                    let mut rows = st.query(params![n as i64, format!("<{}>", sc.graph), pid])?;
                    while rows.next()?.is_some() {
                        k += 1;
                    }
                }
            }
        }
        Ok(k)
    }
    fn write(&mut self, ops: &[WOp], d: Dur) -> Result<Vec<Option<u64>>> {
        self.set_dur(d)?;
        let tx = self.c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut out = Vec::new();
        for op in ops {
            match op {
                WOp::Insert { s, p, o, g } => {
                    out.push(Some(insert_stmt(&tx, s, p, o, g.as_deref())? as u64));
                }
                WOp::Overwrite { s, p, o } => {
                    let (si, pi, oi) = (intern(&tx, s)?, intern(&tx, p)?, intern(&tx, o)?);
                    let olit = matches!(o, Term::Lit { .. }) as i64;
                    let n = tx
                        .prepare_cached("UPDATE stmt SET o = ?3, olit = ?4 WHERE s = ?1 AND p = ?2")?
                        .execute(params![si, pi, oi, olit])?;
                    if n == 0 {
                        tx.prepare_cached("INSERT INTO stmt(s, p, o, olit) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![si, pi, oi, olit])?;
                    }
                }
                WOp::Delete(st) => {
                    if let Some(id) = stmt_id_of(&tx, st)? {
                        if let Some(t) = tx
                            .prepare_cached("SELECT id FROM term WHERE nt = ?1")?
                            .query_row([stmt_term(id as u64)], |r| r.get::<_, i64>(0))
                            .optional()?
                        {
                            tx.prepare_cached("DELETE FROM member WHERE stmt IN (SELECT id FROM stmt WHERE s = ?1)")?
                                .execute([t])?;
                            tx.prepare_cached("DELETE FROM stmt WHERE s = ?1")?.execute([t])?;
                        }
                        tx.prepare_cached("DELETE FROM member WHERE stmt = ?1")?.execute([id])?;
                        tx.prepare_cached("DELETE FROM stmt WHERE id = ?1")?.execute([id])?;
                    }
                }
                WOp::Annotate { target, p, o } => {
                    if let Some(id) = stmt_id_of(&tx, target)? {
                        let s = Term::Iri(format!("urn:glv-bench:stmt:{id}"));
                        insert_stmt(&tx, &s, p, o, None)?;
                    }
                }
                WOp::AddMember { target, g } => {
                    if let Some(id) = stmt_id_of(&tx, target)? {
                        let gid = intern_nt(&tx, &format!("<{g}>"))?;
                        tx.prepare_cached("INSERT OR IGNORE INTO member(g, stmt) VALUES (?1, ?2)")?
                            .execute(params![gid, id])?;
                    }
                }
                WOp::Nest { child, parent } => {
                    let (c, p) = (
                        intern_nt(&tx, &format!("<{child}>"))?,
                        intern_nt(&tx, &format!("<{parent}>"))?,
                    );
                    tx.prepare_cached("INSERT OR IGNORE INTO nest(child, parent) VALUES (?1, ?2)")?
                        .execute(params![c, p])?;
                }
            }
        }
        tx.commit()?;
        Ok(out)
    }
    fn load(&mut self, quads: &[Quad]) -> Result<()> {
        self.set_dur(Dur::Relaxed)?;
        for chunk in quads.chunks(50_000) {
            let tx = self.c.transaction()?;
            for q in chunk {
                insert_stmt(&tx, &q.s, &q.p, &q.o, q.g.as_deref())?;
            }
            tx.commit()?;
        }
        Ok(())
    }
    fn sparql(&mut self, _q: &str) -> Result<usize> {
        unsupported("SPARQL on SQLite")
    }
    fn range_gt(&mut self, p: &Term, lo: &Term) -> Result<usize> {
        let Some(pid) = self.lookup(&p.nt())? else {
            return Ok(0);
        };
        let lo = lo.num().unwrap_or(f64::NEG_INFINITY);
        let mut st = self.c.prepare_cached(
            "SELECT st.s FROM stmt st JOIN term t ON t.id = st.o WHERE st.p = ?1 AND t.num > ?2",
        )?;
        let mut rows = st.query(params![pid, lo])?;
        let mut k = 0;
        while rows.next()?.is_some() {
            k += 1;
        }
        Ok(k)
    }
}
