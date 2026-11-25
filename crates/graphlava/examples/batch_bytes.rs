//! Bytes written to the file per request at group-commit batch sizes 1, 8 and
//! 64 ([[17-benchmarks#Durability commit latency]], [[adr:0025]], task 6.3).
//!
//! `b` threads each submit one `ordered` request of one statement at the same
//! moment, so the requests share one batch (one engine transaction, one
//! commit). Bytes are counted by the I/O-counting backend.
//!
//! Run: `cargo run --release --example batch_bytes`

use std::sync::{Arc, Barrier};
use std::time::Duration;

use graphlava::{
    CoreOp, CrashMode, Database, Durability, FaultPlan, OpenOptions, Term, WriteRequest,
};

const ROUNDS: usize = 64;

fn run(batch: usize) -> (f64, f64) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.glv");
    let count = dir.path().join("count");
    // Preload so the trees have realistic depth.
    {
        let db = Database::open(&path, OpenOptions::default()).unwrap();
        let ops = (0..20_000)
            .map(|i| CoreOp::Insert {
                s: Term::iri(format!("http://b/s{i}")).into(),
                p: Term::iri("http://b/p").into(),
                o: Term::iri(format!("http://b/o{}", i % 97)).into(),
                graph: None,
            })
            .collect();
        db.submit(WriteRequest { ops }, Durability::Sync).unwrap();
        db.close().unwrap();
    }
    let opts = OpenOptions {
        group_window: Duration::from_millis(if batch == 1 { 0 } else { 20 }),
        fault: Some(FaultPlan {
            abort_at: 0,
            mode: CrashMode::ProcessKill,
            count_file: Some(count.clone()),
        }),
        ..OpenOptions::default()
    };
    let db = Arc::new(Database::open(&path, opts).unwrap());
    let mut batches = std::collections::HashSet::new();
    for r in 0..ROUNDS {
        let gate = Arc::new(Barrier::new(batch));
        let hs: Vec<_> = (0..batch)
            .map(|t| {
                let (db, gate) = (Arc::clone(&db), Arc::clone(&gate));
                std::thread::spawn(move || {
                    gate.wait();
                    let op = CoreOp::Insert {
                        s: Term::iri(format!("http://b/r{r}t{t}")).into(),
                        p: Term::iri("http://b/q").into(),
                        o: Term::iri(format!("http://b/o{t}")).into(),
                        graph: None,
                    };
                    db.submit(WriteRequest { ops: vec![op] }, Durability::Ordered)
                        .unwrap()
                        .batch
                })
            })
            .collect();
        for h in hs {
            batches.insert(h.join().unwrap());
        }
    }
    Arc::try_unwrap(db).unwrap().close().unwrap();
    let txt = std::fs::read_to_string(&count).unwrap();
    let bytes: f64 = txt.lines().nth(1).unwrap().parse().unwrap();
    let requests = (ROUNDS * batch) as f64;
    (bytes / requests, requests / batches.len() as f64)
}

fn main() {
    println!(
        "| requests per batch (target) | requests per batch (achieved) | bytes written per request |"
    );
    println!("| --- | --- | --- |");
    for b in [1, 8, 64] {
        let (per_req, achieved) = run(b);
        println!("| {b} | {achieved:.1} | {per_req:.0} |");
    }
}
