//! graphlava: a mutable, single-file metagraph database for agent memory.
//!
//! The design lives in `tg.md/` (start at `tg.md/00-overview.md`). This crate
//! is the embeddable core: term identity, the statement store,
//! named and nested graphs, hyperedges and transactions on redb. Query surfaces arrive in later slices.
//!
//! ```no_run
//! use graphlava::{Database, Durability, OpenOptions, Term};
//! let db = Database::open("memory.glv", OpenOptions::default())?;
//! let mut w = db.write(Durability::Ordered)?;
//! let id = w.insert(Term::iri("http://ex.org/alice"), Term::iri("http://ex.org/knows"), Term::iri("http://ex.org/bob"))?;
//! w.commit()?;
//! # let _ = id;
//! # Ok::<(), graphlava::Error>(())
//! ```

pub mod datalog;
pub mod db;
pub mod dict;
pub mod error;
pub mod graph;
pub mod hedge;
pub mod id;
pub mod literal;
pub mod query;
pub mod sparql;
pub mod storage;
pub mod store;
pub mod tables;
pub mod term;

pub use db::{
    Admin, CommitTicket, CoreOp, Database, Durability, Lang, OpResult, OpenOptions, ReadTxn,
    Reader, RequestBody, SweepReport, WriteRequest, WriteTxn,
};
pub use error::{Error, Result};
pub use hedge::{HedgeType, HedgeTypeDecl, Hyperedge, RoleSpec};
pub use id::Id;
pub use sparql::update::UpdateReport;
pub use sparql::{QueryOptions, QueryResult};
pub use storage::fault::{CrashMode, FaultPlan};
pub use store::{DeleteMode, Match, PredicateDecl};
pub use tables::Perm;
pub use term::{Literal, Term, TermRef, Triple, TripleTerm};

/// The crate version, as set in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
