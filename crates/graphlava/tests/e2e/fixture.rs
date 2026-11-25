//! Temp-dir fixture: open, close and reopen a real file through the public API only.

use std::path::PathBuf;

use graphlava::{Database, Durability, Id, OpenOptions, Reader, Term};

pub struct Fixture {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    pub opts: OpenOptions,
}

impl Fixture {
    pub fn new() -> Fixture {
        Fixture::with(OpenOptions::default())
    }

    pub fn with(opts: OpenOptions) -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("test.glv");
        Fixture {
            _dir: dir,
            path,
            opts,
        }
    }

    pub fn open(&self) -> Database {
        Database::open(&self.path, self.opts.clone()).expect("open")
    }

    /// Closes `db` and opens the file again.
    pub fn reopen(&self, db: Database) -> Database {
        db.close().expect("close");
        self.open()
    }
}

pub fn iri(local: &str) -> Term {
    Term::iri(format!("http://ex.org/{local}"))
}

pub fn int(v: i64) -> Term {
    Term::integer(v)
}

/// The ID of a term in a fresh snapshot.
pub fn id_of(db: &Database, t: &Term) -> Option<Id> {
    db.read().expect("read").lookup(t).expect("lookup")
}

/// Writes in one ordered transaction and returns the closure's result.
pub fn write<R>(db: &Database, f: impl FnOnce(&mut graphlava::WriteTxn<'_>) -> R) -> R {
    let mut w = db.write(Durability::Ordered).expect("write");
    let r = f(&mut w);
    w.commit().expect("commit");
    r
}

pub fn assert_invariants(db: &Database) {
    let r = db.read().expect("read");
    assert_eq!(
        r.check_invariants().expect("check"),
        None,
        "cross-table invariants"
    );
}
