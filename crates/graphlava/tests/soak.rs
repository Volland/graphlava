//! Soak against the model ([[21-e2e-tests#Soak against the model]]).
//!
//! A writer runs a random agent-memory mix (inserts, overwrites, annotations,
//! deletes) in `sync`, `ordered` and `relaxed` requests while 4 readers
//! check that no transaction is ever visible in part. An in-memory model of
//! the expected statements is compared with the store on sampled subjects,
//! and the cross-table invariants are checked at every reopen. Every
//! `GRAPHLAVA_SOAK_REOPEN_SECS` (default 1/6 of the run) the file is swept
//! and reopened.
//!
//! `GRAPHLAVA_SOAK_SECS` sets the duration (default 5 s; the nightly job
//! runs 1800) and `GRAPHLAVA_SOAK_SUBJECTS` the key space (default 2,000;
//! the nightly job uses 250,000 subjects, about 10^6 statements).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use graphlava::{
    CoreOp, Database, DeleteMode, Durability, OpenOptions, Reader, Term, WriteRequest,
};

fn env(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

fn iri(s: &str) -> Term {
    Term::iri(format!("http://soak/{s}"))
}

/// The model: per subject, the "pair" value written to `:a` and `:b` in one
/// transaction, and the employer statement.
#[derive(Default)]
struct Model {
    pair: HashMap<u64, u64>,
    works: HashMap<u64, u64>,
}

// @tg: verifies:: [[21-e2e-tests#Soak against the model]] {level: e2e}
#[test]
fn soak_against_the_model() {
    let secs = env("GRAPHLAVA_SOAK_SECS", 5);
    let subjects = env("GRAPHLAVA_SOAK_SUBJECTS", 2_000);
    let reopen_every = Duration::from_secs(env("GRAPHLAVA_SOAK_REOPEN_SECS", (secs / 6).max(1)));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("soak.glv");
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut model = Model::default();
    let mut x: u64 = 0x5eed;
    let mut next = |m: u64| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) % m
    };
    let mut ops_done = 0u64;
    let mut epochs = 0;
    while Instant::now() < deadline {
        epochs += 1;
        let db = Arc::new(Database::open(&path, OpenOptions::default()).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicU64::new(0));
        // Readers: `:a` and `:b` of a subject are always written together, so
        // a snapshot that sees different values saw half a transaction.
        let readers: Vec<_> = (0..4)
            .map(|t| {
                let (db, stop, reads) = (db.clone(), stop.clone(), reads.clone());
                std::thread::spawn(move || {
                    let mut y: u64 = 99 + t;
                    while !stop.load(Ordering::Relaxed) {
                        y = y.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let s = (y >> 33) % subjects;
                        let r = db.read().unwrap();
                        let Some(sid) = r.lookup(&iri(&format!("s{s}"))).unwrap() else {
                            continue;
                        };
                        let val = |p: &str| -> Option<Term> {
                            let pid = r.lookup(&iri(p)).unwrap()?;
                            r.out(sid, Some(pid))
                                .unwrap()
                                .first()
                                .map(|m| r.decode(m.o).unwrap())
                        };
                        assert_eq!(
                            val("a"),
                            val("b"),
                            "a reader saw half a transaction on s{s}"
                        );
                        reads.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        let epoch_end = (Instant::now() + reopen_every).min(deadline);
        while Instant::now() < epoch_end {
            let s = next(subjects);
            let subj = iri(&format!("s{s}"));
            let durability = [Durability::Relaxed, Durability::Ordered, Durability::Sync]
                [next(10).min(2) as usize];
            let ops = match next(4) {
                // write the pair in one transaction
                0 | 1 => {
                    let v = next(1_000_000) as i64;
                    model.pair.insert(s, v as u64);
                    ["a", "b"]
                        .iter()
                        .map(|p| CoreOp::Set {
                            s: subj.clone().into(),
                            p: iri(p).into(),
                            o: Term::integer(v).into(),
                        })
                        .collect()
                }
                // employer overwrite with an annotation
                2 => {
                    let c = next(50);
                    model.works.insert(s, c);
                    vec![CoreOp::Set {
                        s: subj.clone().into(),
                        p: iri("worksAt").into(),
                        o: iri(&format!("c{c}")).into(),
                    }]
                }
                // delete the employer statement (cascades to annotations)
                _ => {
                    model.works.remove(&s);
                    vec![CoreOp::Remove {
                        s: subj.clone().into(),
                        p: iri("worksAt").into(),
                        o: iri("unused").into(),
                        mode: DeleteMode::Cascade,
                        graph: None,
                    }]
                }
            };
            // `Remove` of a missing triple is a no-op; to delete the employer we
            // remove whatever value it has.
            let ops = if let [CoreOp::Remove { .. }] = ops.as_slice() {
                let r = db.read().unwrap();
                let cur = r.lookup(&subj).unwrap().and_then(|sid| {
                    let p = r.lookup(&iri("worksAt")).unwrap()?;
                    r.out(sid, Some(p)).unwrap().first().map(|m| m.id.unwrap())
                });
                match cur {
                    Some(id) => vec![CoreOp::Delete {
                        id,
                        mode: DeleteMode::Cascade,
                    }],
                    None => continue,
                }
            } else {
                ops
            };
            let res = db.submit(WriteRequest { ops }, durability).unwrap();
            // annotate a fresh employer statement now and then
            if let [graphlava::OpResult::Id(Some(id))] = res.results.as_slice() {
                if next(3) == 0 {
                    db.submit(
                        WriteRequest {
                            ops: vec![CoreOp::Annotate {
                                id: *id,
                                p: iri("confidence").into(),
                                o: Term::double(next(100) as f64 / 100.0).into(),
                            }],
                        },
                        Durability::Relaxed,
                    )
                    .unwrap();
                }
            }
            ops_done += 1;
        }
        stop.store(true, Ordering::Relaxed);
        for r in readers {
            r.join().expect("reader thread");
        }
        // Sweep, then close and reopen; check invariants and the model.
        db.admin().sweep(Duration::ZERO).unwrap();
        Arc::try_unwrap(db).unwrap().close().unwrap();
        let db = Database::open(&path, OpenOptions::default()).unwrap();
        {
            let r = db.read().unwrap();
            assert_eq!(
                r.check_invariants().unwrap(),
                None,
                "invariants after epoch {epochs}"
            );
            for _ in 0..200 {
                let s = next(subjects);
                let Some(sid) = r.lookup(&iri(&format!("s{s}"))).unwrap() else {
                    assert!(!model.pair.contains_key(&s) && !model.works.contains_key(&s));
                    continue;
                };
                let get = |p: &str| -> Vec<Term> {
                    match r.lookup(&iri(p)).unwrap() {
                        Some(pid) => r
                            .out(sid, Some(pid))
                            .unwrap()
                            .iter()
                            .map(|m| r.decode(m.o).unwrap())
                            .collect(),
                        None => Vec::new(),
                    }
                };
                let want_pair: Vec<Term> = model
                    .pair
                    .get(&s)
                    .map(|v| Term::integer(*v as i64))
                    .into_iter()
                    .collect();
                assert_eq!(get("a"), want_pair, "s{s} :a");
                assert_eq!(get("b"), want_pair, "s{s} :b");
                let want_w: Vec<Term> = model
                    .works
                    .get(&s)
                    .map(|c| iri(&format!("c{c}")))
                    .into_iter()
                    .collect();
                assert_eq!(get("worksAt"), want_w, "s{s} :worksAt");
            }
        }
        db.close().unwrap();
        eprintln!(
            "soak epoch {epochs}: {ops_done} write requests, {} reads",
            reads.load(Ordering::Relaxed)
        );
    }
}
