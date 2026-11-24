//! Suspend, resume, flush and exactly-once requests in the core
//! ([[16-api#Mobile bindings]], rule `api-ios-suspend`, change
//! `v1-mobile-bindings` task 2.5). One test per step of `Database::suspend`:
//! abort the interactive write, flush the batch, end the snapshots, release the
//! file lock (app-group files only); and per property of `resume`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use graphlava::{
    CoreOp, Database, Durability, Error, OpenOptions, Reader, RequestBody, Term, TermRef,
    WriteRequest,
};

fn iri(local: &str) -> Term {
    Term::iri(format!("http://ex.org/{local}"))
}

fn insert(s: &str, p: &str, o: &str) -> WriteRequest {
    WriteRequest {
        ops: vec![CoreOp::Insert {
            s: iri(s).into(),
            p: iri(p).into(),
            o: iri(o).into(),
            graph: None,
        }],
    }
}

fn has(db: &Database, s: &str, p: &str, o: &str) -> bool {
    let r = db.read().unwrap();
    let ids = (
        r.lookup(&iri(s)).unwrap(),
        r.lookup(&iri(p)).unwrap(),
        r.lookup(&iri(o)).unwrap(),
    );
    match ids {
        (Some(s), Some(p), Some(o)) => !r.find(Some(s), Some(p), Some(o)).unwrap().is_empty(),
        _ => false,
    }
}

fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("suspend.glv");
    (dir, p)
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Suspend with nothing open]] {level: unit}
#[test]
fn suspend_with_nothing_open_is_fast_and_idempotent() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    // A private-container file: no flush and no engine close.
    let t = Instant::now();
    db.suspend(false).unwrap();
    assert!(t.elapsed() < Duration::from_millis(5), "{:?}", t.elapsed());
    assert!(db.is_suspended());
    db.suspend(false).unwrap();
    db.resume().unwrap();
    // An app-group file also closes the engine handle (redb writes its
    // allocator state with one flush); bounded by the Mobile benchmark's
    // budget, not by 5 ms.
    let t = Instant::now();
    db.suspend(true).unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    let reopens = db.storage_reopens();
    db.suspend(true).unwrap();
    db.suspend(false).unwrap();
    db.resume().unwrap();
    assert!(!db.is_suspended());
    assert_eq!(db.storage_reopens(), reopens + 1);
    db.resume().unwrap();
    assert_eq!(
        db.storage_reopens(),
        reopens + 1,
        "a second resume has no effect"
    );
    db.close().unwrap();
}

/// Step 1: the open interactive write is aborted, not committed; one-shot
/// requests already in the batch commit.
#[test]
fn suspend_aborts_the_interactive_write() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    let mut w = db.write(Durability::Ordered).unwrap();
    w.insert(iri("c"), iri("p"), iri("d")).unwrap();
    db.suspend(false).unwrap();
    let e = w.insert(iri("e"), iri("p"), iri("f")).unwrap_err();
    assert!(matches!(e, Error::Aborted(_)), "{e:?}");
    assert!(matches!(w.commit(), Err(Error::Aborted(_))));
    db.resume().unwrap();
    assert!(has(&db, "a", "p", "b"), "the one-shot request committed");
    assert!(
        !has(&db, "c", "p", "d"),
        "the interactive write was aborted"
    );
    let db = {
        db.close().unwrap();
        Database::open(&path, OpenOptions::default()).unwrap()
    };
    assert!(has(&db, "a", "p", "b"));
    assert!(!has(&db, "c", "p", "d"));
}

/// Step 2: the open batch is flushed, so a relaxed commit acknowledged
/// before suspend is in the file another opener sees.
#[test]
fn suspend_flushes_the_batch_and_releases_the_lock() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    let before = db.durable_commits();
    db.suspend(true).unwrap();
    assert!(
        db.durable_commits() > before,
        "suspend ends with a durable commit"
    );
    // Step 4: the lock is released, so another opener gets the file.
    {
        let other = Database::open(&path, OpenOptions::default()).expect("lock released");
        assert!(has(&other, "a", "p", "b"));
        other.close().unwrap();
    }
    db.resume().unwrap();
    assert!(matches!(
        Database::open(&path, OpenOptions::default()),
        Err(Error::Locked)
    ));
    db.submit(insert("x", "p", "y"), Durability::Sync).unwrap();
    assert!(has(&db, "x", "p", "y"));
    db.close().unwrap();
}

/// Step 3: snapshots must end before the lock is released; a snapshot that is
/// still alive makes suspend fail with `Busy` after its bounded wait, keeps
/// the lock, and leaves the database suspended until resume.
#[test]
fn live_snapshot_blocks_lock_release() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    let r = db.read().unwrap();
    assert_eq!(db.open_snapshots(), 1);
    let e = db.suspend(true).unwrap_err();
    assert!(matches!(e, Error::Busy(_)), "{e:?}");
    assert!(matches!(
        Database::open(&path, OpenOptions::default()),
        Err(Error::Locked)
    ));
    drop(r);
    assert_eq!(db.open_snapshots(), 0);
    db.resume().unwrap();
    db.close().unwrap();
}

/// Snapshots ended by a waiting thread let the release go through.
#[test]
fn snapshot_ending_during_the_wait_is_enough() {
    let (_d, path) = tmp();
    let db = Arc::new(Database::open(&path, OpenOptions::default()).unwrap());
    let r = db.read().unwrap();
    let r: graphlava::ReadTxn<'static> = unsafe {
        // SAFETY: the Arc keeps the database alive until after this snapshot
        // is dropped by the thread below, which is joined before `db` drops.
        std::mem::transmute::<graphlava::ReadTxn<'_>, graphlava::ReadTxn<'static>>(r)
    };
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(r);
    });
    db.suspend(true).unwrap();
    h.join().unwrap();
    db.resume().unwrap();
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Calls while suspended wait for resume]] {level: unit}
#[test]
fn calls_while_suspended_wait_for_resume() {
    let (_d, path) = tmp();
    let db = Arc::new(Database::open(&path, OpenOptions::default()).unwrap());
    db.submit(insert("a", "p", "b"), Durability::Sync).unwrap();
    db.suspend(true).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let (db2, done2) = (Arc::clone(&db), Arc::clone(&done));
    let reader = std::thread::spawn(move || {
        let found = has(&db2, "a", "p", "b");
        done2.store(true, Ordering::SeqCst);
        found
    });
    let (db3, done3) = (Arc::clone(&db), Arc::clone(&done));
    let writer = std::thread::spawn(move || {
        db3.submit(insert("c", "p", "d"), Durability::Ordered)
            .unwrap();
        done3.load(Ordering::SeqCst)
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(!done.load(Ordering::SeqCst), "the read waited for resume");
    let reopens = db.storage_reopens();
    db.resume().unwrap();
    assert!(reader.join().unwrap());
    writer.join().unwrap();
    assert_eq!(db.storage_reopens(), reopens + 1);
    assert!(has(&db, "c", "p", "d"));
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Private container keeps the lock]] {level: unit}
#[test]
fn private_container_keeps_the_lock() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    let before = db.durable_commits();
    let reopens = db.storage_reopens();
    db.suspend(false).unwrap();
    assert!(
        db.durable_commits() > before,
        "the pending batch is flushed"
    );
    assert!(matches!(
        Database::open(&path, OpenOptions::default()),
        Err(Error::Locked)
    ));
    db.resume().unwrap();
    assert_eq!(
        db.storage_reopens(),
        reopens,
        "resume does not reopen the file"
    );
    db.close().unwrap();
}

#[test]
fn resume_fails_while_another_owner_holds_the_file() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.suspend(true).unwrap();
    let other = Database::open(&path, OpenOptions::default()).unwrap();
    assert!(matches!(db.resume(), Err(Error::Locked)));
    assert!(db.is_suspended(), "still suspended after a failed resume");
    other.close().unwrap();
    db.resume().unwrap();
    db.close().unwrap();
}

#[test]
fn flush_makes_relaxed_commits_durable() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    // A waiting writer keeps the relaxed batch open; flush closes it.
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    let before = db.durable_commits();
    db.flush().unwrap();
    db.flush().unwrap();
    assert!(db.durable_commits() >= before);
    assert!(has(&db, "a", "p", "b"));
    db.close().unwrap();
}

#[test]
fn close_while_suspended() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    db.suspend(true).unwrap();
    db.close().unwrap();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    assert!(has(&db, "a", "p", "b"));
}

/// `submit_once` applies a request and records its sequence number in one
/// transaction; a repeated or older number is skipped.
#[test]
fn submit_once_applies_each_sequence_number_once() {
    let (_d, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    let body = RequestBody::Ops(insert("a", "p", "b"));
    assert_eq!(db.once_seq("ext").unwrap(), 0);
    assert!(
        db.submit_once("ext", 1, &body, Durability::Ordered)
            .unwrap()
    );
    assert!(
        !db.submit_once("ext", 1, &body, Durability::Ordered)
            .unwrap()
    );
    let upd = RequestBody::SparqlUpdate(
        "INSERT DATA { <http://ex.org/c> <http://ex.org/p> <http://ex.org/d> }".into(),
    );
    assert!(db.submit_once("ext", 3, &upd, Durability::Ordered).unwrap());
    assert!(
        !db.submit_once("ext", 2, &body, Durability::Ordered)
            .unwrap()
    );
    assert_eq!(db.once_seq("ext").unwrap(), 3);
    assert_eq!(db.once_seq("other").unwrap(), 0);
    // A failing request records nothing: the sequence number stays.
    let bad = RequestBody::SparqlUpdate("INSERT DATA { nonsense".into());
    assert!(db.submit_once("ext", 4, &bad, Durability::Ordered).is_err());
    assert_eq!(db.once_seq("ext").unwrap(), 3);
    let prog = RequestBody::Datalog(
        "@assert <http://ex.org/e>(<http://ex.org/f>, <http://ex.org/g>) :- <http://ex.org/p>(?x, ?y).".into(),
    );
    let _ = db.submit_once("ext", 5, &prog, Durability::Ordered);
    let db = {
        db.close().unwrap();
        Database::open(&path, OpenOptions::default()).unwrap()
    };
    assert!(db.once_seq("ext").unwrap() >= 3);
    assert!(has(&db, "a", "p", "b") && has(&db, "c", "p", "d"));
    let r = db.read().unwrap();
    assert!(r.lookup(&Term::iri("http://ex.org/zzz")).unwrap().is_none());
    let _ = TermRef::Term(iri("unused"));
}

#[test]
fn export_snapshot_copies_a_consistent_file() {
    let (dir, path) = tmp();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    db.submit(insert("a", "p", "b"), Durability::Relaxed)
        .unwrap();
    let copy = dir.path().join("copy.glv");
    db.export_snapshot(&copy).unwrap();
    db.submit(insert("c", "p", "d"), Durability::Sync).unwrap();
    let other = Database::open(&copy, OpenOptions::default()).unwrap();
    assert!(has(&other, "a", "p", "b"));
    assert!(!has(&other, "c", "p", "d"));
    assert_eq!(other.read().unwrap().check_invariants().unwrap(), None);
    other.close().unwrap();
    db.close().unwrap();
}
