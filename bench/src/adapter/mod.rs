//! The harness trait: one adapter per engine, one session per thread
//! ([[17-benchmarks#Harness and method]]). Every workload talks to engines only
//! through [`Adapter`] and [`Session`].
//!
//! - [`Adapter`] owns the open database and creates per-thread sessions
//!   (SQLite needs one connection per thread; the others share one handle).
//! - [`Session`] has two levels: the agent-memory primitives (node lookup,
//!   properties, statement by ID, neighbours, scoped reads, write batches) and
//!   the standard-workload surface (bulk load of RDF quads, SPARQL text,
//!   Datalog text). An engine that lacks a surface returns
//!   [`Unsupported`]; the workload records the gap instead of faking a number.

pub mod graphlava;
pub mod oxigraph;
pub mod sqlite;
#[cfg(feature = "tiramemsu")]
pub mod tiramemsu;

use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;

/// Durability levels of tg.md/06; each adapter maps them to its closest setting
/// and reports the mapping in [`Adapter::describe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dur {
    Sync,
    Ordered,
    Relaxed,
}

impl Dur {
    pub fn name(self) -> &'static str {
        match self {
            Dur::Sync => "sync",
            Dur::Ordered => "ordered",
            Dur::Relaxed => "relaxed",
        }
    }
}

/// A generic RDF term (standard workloads and generated data).
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Iri(String),
    Blank(String),
    Lit {
        lex: String,
        dt: String,
        lang: Option<String>,
    },
}

pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

impl Term {
    pub fn int(v: i64) -> Term {
        Term::Lit {
            lex: v.to_string(),
            dt: format!("{XSD}integer"),
            lang: None,
        }
    }
    pub fn string(s: impl Into<String>) -> Term {
        Term::Lit {
            lex: s.into(),
            dt: format!("{XSD}string"),
            lang: None,
        }
    }
    pub fn typed(lex: impl Into<String>, dt_local: &str) -> Term {
        Term::Lit {
            lex: lex.into(),
            dt: format!("{XSD}{dt_local}"),
            lang: None,
        }
    }
    /// N-Triples form (SQLite dictionary key, debugging).
    pub fn nt(&self) -> String {
        match self {
            Term::Iri(i) => format!("<{i}>"),
            Term::Blank(b) => format!("_:{b}"),
            Term::Lit { lex, dt, lang } => {
                let esc = lex
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                match lang {
                    Some(l) => format!("\"{esc}\"@{l}"),
                    None if dt == &format!("{XSD}string") => format!("\"{esc}\""),
                    None => format!("\"{esc}\"^^<{dt}>"),
                }
            }
        }
    }
    /// Numeric value of a numeric or dateTime-free literal (SQLite range column).
    pub fn num(&self) -> Option<f64> {
        match self {
            Term::Lit { lex, dt, .. } => {
                let local = dt.strip_prefix(XSD)?;
                match local {
                    "integer" | "int" | "long" | "decimal" | "double" | "float"
                    | "nonNegativeInteger" | "positiveInteger" => lex.trim().parse().ok(),
                    "dateTime" => date_time_secs(lex),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Seconds since 1970 of an `xsd:dateTime` `YYYY-MM-DDThh:mm:ss[.f][Z]`
/// (timezone offsets other than Z are ignored).
fn date_time_secs(lex: &str) -> Option<f64> {
    let (d, t) = lex.split_once('T')?;
    let mut dp = d.split('-');
    let (y, m, dd): (i64, i64, i64) = (dp.next()?.parse().ok()?, dp.next()?.parse().ok()?, dp.next()?.parse().ok()?);
    let t = t.trim_end_matches('Z');
    let mut tp = t.split(':');
    let (h, mi): (i64, i64) = (tp.next()?.parse().ok()?, tp.next()?.parse().ok()?);
    let sec: f64 = tp.next()?.get(..2)?.parse().ok()?;
    // Days from civil (H. Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60) as f64 + sec)
}

/// A statement of the standard workloads: `g` is a named graph IRI.
#[derive(Clone, Debug)]
pub struct Quad {
    pub s: Term,
    pub p: Term,
    pub o: Term,
    pub g: Option<String>,
}

/// A statement the harness created, so it can address it again: the engine's
/// own handle where it has one (graphlava statement ID, SQLite rowid,
/// tiramemsu eid) and the triple for engines without statement IDs (Oxigraph).
#[derive(Clone, Debug)]
pub struct StmtRef {
    pub native: Option<u64>,
    pub s: Term,
    pub p: Term,
    pub o: Term,
    /// Home graph (kept for adapters that address statements per graph).
    #[allow(dead_code)]
    pub g: Option<String>,
}

/// A read scope: a graph IRI plus the episode graphs it covers, for engines
/// without nested graphs (they union the episodes).
#[derive(Clone, Debug)]
pub struct Scope {
    pub graph: String,
    pub episodes: Vec<String>,
}

/// One operation of a write transaction.
#[derive(Clone, Debug)]
pub enum WOp {
    /// Insert into graph `g` (an episode); returns a [`StmtRef`].
    Insert { s: Term, p: Term, o: Term, g: Option<String> },
    /// Replace every value of `(s, p)` by `o` (value overwrite).
    Overwrite { s: Term, p: Term, o: Term },
    /// Delete a statement (cascading to its annotations where the engine has them).
    Delete(StmtRef),
    /// A statement about a statement.
    Annotate { target: StmtRef, p: Term, o: Term },
    /// Make an existing fact a member of a second graph.
    AddMember { target: StmtRef, g: String },
    /// Nest graph `child` under `parent` (load-time setup).
    Nest { child: String, parent: String },
}

/// Engine-specific handle of a node (graphlava ID, SQLite term id,
/// tiramemsu ObjectId, or the generated index for Oxigraph).
pub type NodeH = u64;

#[derive(Debug)]
pub struct Unsupported(pub &'static str);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported by this adapter: {}", self.0)
    }
}
impl std::error::Error for Unsupported {}

pub fn unsupported<T>(what: &'static str) -> Result<T> {
    Err(Unsupported(what).into())
}

pub fn is_unsupported(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Unsupported>().is_some()
}

/// Per-thread access to an open engine.
pub trait Session {
    /// The handle of a node IRI, `None` if absent (includes the lookup cost).
    fn node(&mut self, iri: &str) -> Result<Option<NodeH>>;
    /// Number of literal-valued properties of a node.
    fn props(&mut self, n: NodeH) -> Result<usize>;
    /// Reads a statement by its handle; whether it exists.
    fn stmt(&mut self, st: &StmtRef) -> Result<bool>;
    /// Node-valued objects of `n`, at most `limit`, optionally inside a scope.
    fn out(&mut self, n: NodeH, limit: usize, scope: Option<&Scope>) -> Result<Vec<NodeH>>;
    /// Values of the given predicates of `n` (a star pattern); rows found.
    fn star(&mut self, n: NodeH, preds: &[Term], scope: Option<&Scope>) -> Result<usize>;
    /// Applies a batch as one transaction at `dur`; one result per `Insert`
    /// (in order) carrying the engine handle.
    fn write(&mut self, ops: &[WOp], dur: Dur) -> Result<Vec<Option<u64>>>;

    /// Bulk-loads quads (standard workloads), not timed per statement.
    fn load(&mut self, quads: &[Quad]) -> Result<()>;
    /// Runs a SPARQL query; returns the number of result rows (1 for ASK true).
    fn sparql(&mut self, q: &str) -> Result<usize> {
        let _ = q;
        unsupported("SPARQL")
    }
    /// Runs a SPARQL update at `dur`.
    fn sparql_update(&mut self, u: &str, dur: Dur) -> Result<()> {
        let _ = (u, dur);
        unsupported("SPARQL update")
    }
    /// Runs a read-only Datalog program; returns result rows.
    fn datalog(&mut self, program: &str) -> Result<usize> {
        let _ = program;
        unsupported("Datalog")
    }
    /// Values of `p` with `lo < v` (numeric), returns rows (range scans).
    fn range_gt(&mut self, p: &Term, lo: &Term) -> Result<usize> {
        let _ = (p, lo);
        unsupported("range scan")
    }
}

/// An open engine.
pub trait Adapter: Send + Sync {
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
    /// Version, storage backend and the durability mapping in force.
    fn describe(&self) -> Value;
    fn session(&self) -> Result<Box<dyn Session + '_>>;
    /// Bytes on disk (file or directory).
    fn disk_size(&self) -> u64;
    /// Makes everything durable (end of a load phase).
    fn checkpoint(&self) -> Result<()>;
    /// Engine-specific extras (graphlava admin operations).
    fn as_graphlava(&self) -> Option<&graphlava::GraphlavaAdapter> {
        None
    }
}

/// Which engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Engine {
    Graphlava,
    Sqlite,
    Oxigraph,
    Tiramemsu,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Graphlava => "graphlava",
            Engine::Sqlite => "sqlite",
            Engine::Oxigraph => "oxigraph",
            Engine::Tiramemsu => "tiramemsu",
        }
    }
}

/// Engine options shared by all adapters.
#[derive(Clone, Debug)]
pub struct OpenCfg {
    pub dir: PathBuf,
    /// Durability used for commits whose level the workload does not choose.
    pub dur: Dur,
    /// graphlava group-commit window (ms).
    pub group_window_ms: u64,
    pub cache_bytes: usize,
}

pub fn open(e: Engine, cfg: &OpenCfg) -> Result<Box<dyn Adapter>> {
    std::fs::create_dir_all(&cfg.dir)?;
    Ok(match e {
        Engine::Graphlava => Box::new(graphlava::GraphlavaAdapter::open(cfg)?),
        Engine::Sqlite => Box::new(sqlite::SqliteAdapter::open(cfg)?),
        Engine::Oxigraph => Box::new(oxigraph::OxigraphAdapter::open(cfg)?),
        #[cfg(feature = "tiramemsu")]
        Engine::Tiramemsu => Box::new(tiramemsu::TiramemsuAdapter::open(cfg)?),
        #[cfg(not(feature = "tiramemsu"))]
        Engine::Tiramemsu => anyhow::bail!(
            "tiramemsu adapter not compiled: build with `--features tiramemsu` (Rust 1.88+)"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_time_numbers() {
        assert_eq!(date_time_secs("1970-01-02T00:00:00Z"), Some(86_400.0));
        let a = Term::typed(crate::suite::tasks::date_time(5), "dateTime").num().unwrap();
        let b = Term::typed(crate::suite::tasks::date_time(6), "dateTime").num().unwrap();
        assert_eq!(b - a, 1.0);
    }
}
