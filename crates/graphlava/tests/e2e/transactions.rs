//! Scenarios of `specs/transactions/spec.md` that need no crash; the crash
//! scenarios are in `tests/crash.rs`.

use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use graphlava::{CoreOp, Database, Durability, Error, OpenOptions, Reader, WriteRequest};

use crate::fixture::*;

const CHILD_OPEN: &str = "GRAPHLAVA_TEST_CHILD_OPEN";

/// Child half of `second_process_is_refused`: tries to open the file.
#[test]
fn child_open_probe() {
    let Ok(path) = std::env::var(CHILD_OPEN) else {
        return;
    };
    let t = std::time::Instant::now();
    match Database::open(&path, OpenOptions::default()) {
        Err(Error::Locked) => println!("CHILD:LOCKED {}", t.elapsed().as_micros()),
        Err(e) => println!("CHILD:ERROR {e}"),
        Ok(_) => println!("CHILD:OPENED"),
    }
}

// @tg: verifies:: [[openspec:transactions#Single owner process#Second process is refused]] {level: e2e}
// @tg: verifies:: [[21-e2e-tests#Second process is refused]] {level: e2e}
#[test]
fn second_process_is_refused() {
    let f = Fixture::with(OpenOptions {
        write_timeout: Duration::from_secs(30),
        ..OpenOptions::default()
    });
    let db = f.open();
    write(&db, |w| w.insert(iri("a"), iri("p"), iri("b")).unwrap());
    // A holds an active write transaction while B tries.
    let mut active = db.write(Durability::Ordered).unwrap();
    active.insert(iri("a"), iri("p"), iri("d")).unwrap();
    // Same process, second handle.
    assert!(matches!(
        Database::open(&f.path, OpenOptions::default()),
        Err(Error::Locked)
    ));
    // Another process.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "transactions::child_open_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_OPEN, &f.path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("CHILD:LOCKED"), "child output: {stdout}");
    let micros: u64 = stdout
        .split("CHILD:LOCKED ")
        .nth(1)
        .and_then(|x| x.split_whitespace().next())
        .and_then(|x| x.parse().ok())
        .unwrap();
    assert!(
        micros < 100_000,
        "B waited {micros} µs for the locked error"
    );
    // A commits normally.
    active.commit().unwrap();
    write(&db, |w| w.insert(iri("a"), iri("p"), iri("c")).unwrap());
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(
        r.find(None, r.lookup(&iri("p")).unwrap(), None)
            .unwrap()
            .len(),
        3
    );
}

// @tg: verifies:: [[openspec:transactions#Snapshot reads and one writer#Reader does not see uncommitted writes]] {level: e2e}
#[test]
fn reader_does_not_see_uncommitted_writes() {
    let f = Fixture::new();
    let db = f.open();
    let before = db.read().unwrap();
    let mut w = db.write(Durability::Ordered).unwrap();
    w.insert(iri("a"), iri("p"), iri("b")).unwrap();
    let during = db.read().unwrap();
    assert_eq!(during.lookup(&iri("a")).unwrap(), None);
    w.commit().unwrap();
    assert_eq!(
        before.lookup(&iri("a")).unwrap(),
        None,
        "an old snapshot stays stable"
    );
    assert!(db.read().unwrap().lookup(&iri("a")).unwrap().is_some());
    drop((before, during));
    let db = f.reopen(db);
    assert!(id_of(&db, &iri("a")).is_some());
}

fn slow_window() -> OpenOptions {
    OpenOptions {
        group_window: Duration::from_millis(300),
        ..OpenOptions::default()
    }
}

// @tg: verifies:: [[openspec:transactions#Batched commit#Ordered write is invisible until durable]] {level: e2e}
#[test]
fn ordered_write_is_invisible_until_durable() {
    let f = Fixture::with(slow_window());
    let db = Arc::new(f.open());
    let d = Arc::clone(&db);
    let writer = std::thread::spawn(move || {
        let req = WriteRequest {
            ops: vec![CoreOp::Insert {
                s: iri("a").into(),
                p: iri("p").into(),
                o: iri("b").into(),
                graph: None,
            }],
        };
        d.submit(req, Durability::Ordered).unwrap()
    });
    std::thread::sleep(Duration::from_millis(100));
    // The request has run in the open batch, which is not committed yet.
    assert_eq!(db.read().unwrap().lookup(&iri("a")).unwrap(), None);
    writer.join().unwrap();
    assert!(
        db.read().unwrap().lookup(&iri("a")).unwrap().is_some(),
        "visible after the acknowledgement"
    );
    let db = Arc::try_unwrap(db).unwrap();
    let db = f.reopen(db);
    assert!(id_of(&db, &iri("a")).is_some());
}

// @tg: verifies:: [[openspec:transactions#Batched commit#Failed request does not affect its batch]] {level: e2e}
#[test]
fn failed_request_does_not_affect_its_batch() {
    let f = Fixture::with(slow_window());
    let db = Arc::new(f.open());
    let id = write(&db, |w| {
        w.insert(iri("x"), iri("loc"), iri("rome"))
            .unwrap()
            .unwrap()
    });
    let start = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let (d, b) = (Arc::clone(&db), Arc::clone(&start));
            std::thread::spawn(move || {
                b.wait();
                std::thread::sleep(Duration::from_millis(20 * i as u64));
                let op = if i == 1 {
                    CoreOp::Update {
                        id,
                        o: iri("berlin").into(),
                        expected: Some(iri("paris").into()),
                    }
                } else {
                    CoreOp::Insert {
                        s: iri(&format!("r{i}")).into(),
                        p: iri("p").into(),
                        o: iri("o").into(),
                        graph: None,
                    }
                };
                d.submit(WriteRequest { ops: vec![op] }, Durability::Ordered)
            })
        })
        .collect();
    let res: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(matches!(res[1], Err(Error::Conflict(_))), "{:?}", res[1]);
    let (t0, t2) = (res[0].as_ref().unwrap(), res[2].as_ref().unwrap());
    assert_eq!(
        t0.batch, t2.batch,
        "the first and third request shared one batch"
    );
    let db = Arc::try_unwrap(db).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.lookup(&iri("r0")).unwrap().is_some());
    assert!(r.lookup(&iri("r2")).unwrap().is_some());
    assert_eq!(
        r.decode(r.stmt(id).unwrap().unwrap().o).unwrap(),
        iri("rome")
    );
}

// @tg: verifies:: [[openspec:transactions#One-shot remote writes#Slow in-process writer is cut off]] {level: e2e}
#[test]
fn slow_in_process_writer_is_cut_off() {
    let f = Fixture::with(OpenOptions {
        write_timeout: Duration::from_millis(100),
        ..OpenOptions::default()
    });
    let db = Arc::new(f.open());
    let mut w = db.write(Durability::Ordered).unwrap();
    w.insert(iri("slow"), iri("p"), iri("o")).unwrap();
    // A queued request proceeds once the writer's time is up, without any call from it.
    let d = Arc::clone(&db);
    let queued = std::thread::spawn(move || {
        let req = WriteRequest {
            ops: vec![CoreOp::Insert {
                s: iri("queued").into(),
                p: iri("p").into(),
                o: iri("o").into(),
                graph: None,
            }],
        };
        d.submit(req, Durability::Ordered)
    });
    let queued = queued.join().unwrap();
    assert!(queued.is_ok(), "{queued:?}");
    assert!(matches!(
        w.insert(iri("late"), iri("p"), iri("o")),
        Err(Error::WriteTimeout)
    ));
    assert!(matches!(w.commit(), Err(Error::WriteTimeout)));
    let db = Arc::try_unwrap(db).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.lookup(&iri("queued")).unwrap().is_some());
    assert_eq!(
        r.lookup(&iri("slow")).unwrap(),
        None,
        "the timed-out transaction was aborted"
    );
}

// @tg: verifies:: [[openspec:transactions#Portable file format#File moves between platforms]] {level: e2e}
#[test]
fn file_moves_between_platforms() {
    // The layout has no platform fields (little-endian values, big-endian keys),
    // so a closed file copied byte for byte must answer the same. The cross-OS
    // golden-file job (v0-hardening-freeze) runs the same check across machines.
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        let id = w.insert(iri("a"), iri("p"), iri("b")).unwrap().unwrap();
        w.annotate(id, iri("weight"), graphlava::Term::double(0.25))
            .unwrap();
        w.insert(iri("a"), iri("name"), graphlava::Term::string("Ann"))
            .unwrap();
    });
    db.close().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("moved.glv");
    std::fs::copy(&f.path, &copy).unwrap();
    let a = f.open();
    let b = Database::open(&copy, OpenOptions::default()).unwrap();
    let dump = |db: &Database| {
        let r = db.read().unwrap();
        let mut rows: Vec<String> = r
            .find(None, None, None)
            .unwrap()
            .iter()
            .map(|m| {
                format!(
                    "{} {} {}",
                    r.decode(m.s).unwrap(),
                    r.decode(m.p).unwrap(),
                    r.decode(m.o).unwrap()
                )
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(dump(&a), dump(&b));
    // two stored statements, one inline property, and the virtual
    // `(stmt, :in, defaultGraph)` membership of the stored statement
    assert_eq!(dump(&a).len(), 4);
}

// @tg: verifies:: [[openspec:transactions#Portable file format#Unknown feature flag]] {level: e2e}
#[test]
fn unknown_feature_flag() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| w.insert(iri("a"), iri("p"), iri("b")).unwrap());
    db.close().unwrap();
    {
        // Simulate a file written by a newer build: set a feature bit this build lacks.
        let raw = redb::Database::open(&f.path).unwrap();
        let w = raw.begin_write().unwrap();
        {
            let mut t = w
                .open_table(redb::TableDefinition::<&[u8], &[u8]>::new("meta"))
                .unwrap();
            t.insert(&b"features"[..], &(1u64 << 40).to_le_bytes()[..])
                .unwrap();
        }
        w.commit().unwrap();
    }
    let before = std::fs::read(&f.path).unwrap();
    let err = Database::open(&f.path, OpenOptions::default());
    assert!(matches!(err, Err(Error::FormatMismatch(_))), "{err:?}");
    drop(err);
    assert_eq!(
        std::fs::read(&f.path).unwrap(),
        before,
        "the file is not modified"
    );
}

/// Rule `tx-relaxed-bound`: a long run of relaxed requests forces a durable
/// commit every `max_relaxed_run` batches, so the file does not grow without
/// bound (engine bake-off: redb frees pages only at durable commits).
#[test]
fn relaxed_runs_are_bounded() {
    let f = Fixture::with(OpenOptions {
        max_relaxed_run: 50,
        group_window: std::time::Duration::ZERO,
        ..OpenOptions::default()
    });
    let db = f.open();
    let id = write(&db, |w| {
        w.insert(iri("a"), iri("p"), iri("b0")).unwrap().unwrap()
    });
    let before = db.durable_commits();
    let mut sizes = Vec::new();
    for i in 0..1000 {
        db.submit(
            WriteRequest {
                ops: vec![CoreOp::Update {
                    id,
                    o: iri(&format!("b{}", i % 7)).into(),
                    expected: None,
                }],
            },
            Durability::Relaxed,
        )
        .unwrap();
        if i % 250 == 249 {
            sizes.push(db.file_size().unwrap());
        }
    }
    let forced = db.durable_commits() - before;
    assert!(forced >= 1000 / 50 - 2, "{forced} durable commits forced");
    // overwrites of one statement keep the file size flat once pages recycle
    assert!(sizes[3] <= sizes[1] * 2, "file keeps growing: {sizes:?}");
    assert_invariants(&db);
}
