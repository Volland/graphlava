//! Crash harness ([[19-verification]] L4, [[06-transactions-durability#Crash recovery]]).
//!
//! A child process writes a fixed schedule of transactions through the public
//! API on a database opened with the fault-injecting backend, which aborts the
//! process at the N-th I/O call. The parent reruns the child for every N up to
//! the run's total I/O count, reopens the file, and checks the outcome against
//! `MetaCore.Durability`: the surviving transactions are a prefix
//! (`CrashOutcome`: `durableBound ≤ k ≤ length`), every transaction is all or
//! nothing (`recover` applies whole transactions), and every acknowledged
//! `sync` or `ordered` transaction survives (`durable_survives`).
//!
//! Two crash models run: `ProcessKill` (the OS keeps accepted writes) and
//! `PowerCut` (only synced data survives).

use std::io::Write as _;
use std::process::Command;

use graphlava::{
    CoreOp, CrashMode, Database, Durability, FaultPlan, OpenOptions, Reader, Term, WriteRequest,
};

const ENV_PATH: &str = "GRAPHLAVA_CRASH_PATH";
const ENV_ABORT: &str = "GRAPHLAVA_CRASH_ABORT_AT";
const ENV_MODE: &str = "GRAPHLAVA_CRASH_MODE";
const ENV_COUNT: &str = "GRAPHLAVA_CRASH_COUNT";
const ENV_QUICK: &str = "GRAPHLAVA_CRASH_QUICK";

fn quick() -> bool {
    std::env::var(ENV_QUICK).is_ok()
}

/// The schedule: (statements in the transaction, durability).
fn schedule() -> Vec<(usize, Durability)> {
    let mut s = Vec::new();
    for i in 0..9 {
        let d = [Durability::Relaxed, Durability::Ordered, Durability::Sync][i % 3];
        s.push((3, d));
    }
    // Scenario "Crash during commit": one transaction of 1,000 statements.
    s.insert(4, (1000, Durability::Sync));
    s.push((2, Durability::Relaxed));
    s
}

fn subject(t: usize) -> Term {
    Term::iri(format!("http://crash/t{t}"))
}

fn ops(t: usize, n: usize) -> Vec<CoreOp> {
    (0..n)
        .map(|j| CoreOp::Insert {
            s: subject(t).into(),
            p: Term::iri(format!("http://crash/k{j}")).into(),
            o: Term::iri("http://crash/v").into(),
            graph: None,
        })
        .collect()
}

/// Child half: runs the schedule and prints an `ACK i` line per acknowledged transaction.
#[test]
fn crash_child() {
    let Ok(path) = std::env::var(ENV_PATH) else {
        return;
    };
    let abort_at: u64 = std::env::var(ENV_ABORT).unwrap().parse().unwrap();
    let mode = if std::env::var(ENV_MODE).unwrap() == "power" {
        CrashMode::PowerCut
    } else {
        CrashMode::ProcessKill
    };
    let count_file = std::env::var(ENV_COUNT).ok().map(Into::into);
    let opts = OpenOptions {
        fault: Some(FaultPlan {
            abort_at,
            mode,
            count_file,
        }),
        quick_repair: quick(),
        ..OpenOptions::default()
    };
    let db = Database::open(&path, opts).unwrap();
    let mut out = std::io::stdout();
    for (i, (n, d)) in schedule().into_iter().enumerate() {
        db.submit(WriteRequest { ops: ops(i, n) }, d).unwrap();
        writeln!(out, "ACK {i}").unwrap();
        out.flush().unwrap();
    }
    db.close().unwrap();
}

fn run_child(
    path: &std::path::Path,
    abort_at: u64,
    mode: &str,
    count: Option<&std::path::Path>,
    quick_repair: bool,
) -> (Vec<usize>, bool) {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    if quick_repair {
        cmd.env(ENV_QUICK, "1");
    }
    cmd.args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env(ENV_PATH, path)
        .env(ENV_ABORT, abort_at.to_string())
        .env(ENV_MODE, mode);
    if let Some(c) = count {
        cmd.env(ENV_COUNT, c);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let acks = stdout
        .split("ACK ")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect();
    (acks, out.status.success())
}

/// `MetaCore.Durability.durableBound` over the acknowledged prefix.
fn durable_bound(acked: &[usize], sched: &[(usize, Durability)]) -> usize {
    acked
        .iter()
        .filter(|i| sched[**i].1 != Durability::Relaxed)
        .map(|i| i + 1)
        .max()
        .unwrap_or(0)
}

fn check(path: &std::path::Path, acked: &[usize], label: &str, quick_repair: bool) {
    let sched = schedule();
    let opts = OpenOptions {
        quick_repair,
        ..OpenOptions::default()
    };
    let db = Database::open(path, opts).unwrap_or_else(|e| panic!("{label}: reopen failed: {e}"));
    let r = db.read().unwrap();
    assert_eq!(r.check_invariants().unwrap(), None, "{label}: invariants");
    let mut max_id = graphlava::Id(0);
    let present: Vec<usize> = sched
        .iter()
        .enumerate()
        .map(|(i, (n, _))| {
            let got = match r.lookup(&subject(i)).unwrap() {
                None => 0,
                Some(s) => {
                    let m = r.out(s, None).unwrap();
                    for x in &m {
                        max_id = max_id.max(x.id.unwrap_or_default());
                    }
                    m.len()
                }
            };
            assert!(
                got == 0 || got == *n,
                "{label}: transaction {i} is partial ({got} of {n})"
            );
            usize::from(got == *n)
        })
        .collect();
    // Surviving transactions form a prefix of length k.
    let k = present.iter().take_while(|p| **p == 1).count();
    assert!(
        present[k..].iter().all(|p| *p == 0),
        "{label}: survivors are not a prefix: {present:?}"
    );
    let bound = durable_bound(acked, &sched);
    assert!(
        k >= bound,
        "{label}: lost an acknowledged durable transaction (k = {k}, durableBound = {bound}, acked = {acked:?})"
    );
    drop(r);
    // The allocator is not below any surviving ID (no ID is handed out twice).
    let fresh = db
        .submit(
            WriteRequest {
                ops: vec![CoreOp::Insert {
                    s: Term::iri("http://crash/after").into(),
                    p: Term::iri("http://crash/k").into(),
                    o: Term::iri("http://crash/v").into(),
                    graph: None,
                }],
            },
            Durability::Sync,
        )
        .unwrap();
    let graphlava::OpResult::Id(Some(id)) = fresh.results[0] else {
        panic!("{label}: no ID")
    };
    assert!(
        id > max_id,
        "{label}: fresh ID {id} is not above surviving {max_id}"
    );
    db.close().unwrap();
}

fn sweep(mode: &str) {
    sweep_with(mode, false);
}

fn sweep_with(mode: &str, quick: bool) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.glv");
    Database::open(&base, OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    // Count the I/O calls of a full run.
    let count = dir.path().join("count");
    let probe = dir.path().join("probe.glv");
    std::fs::copy(&base, &probe).unwrap();
    let (acks, ok) = run_child(&probe, 0, mode, Some(&count), quick);
    assert!(
        ok && acks.len() == schedule().len(),
        "uninterrupted run must finish: {acks:?}"
    );
    let total: u64 = std::fs::read_to_string(&count)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(total > 0);
    let limit: u64 = std::env::var("GRAPHLAVA_CRASH_POINTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let step = total.div_ceil(limit.min(total)).max(1);
    let mut points = 0;
    for n in (1..=total).step_by(step as usize) {
        let file = dir.path().join(format!("{mode}-{n}.glv"));
        std::fs::copy(&base, &file).unwrap();
        let (acks, _) = run_child(&file, n, mode, None, quick);
        check(
            &file,
            &acks,
            &format!("{mode} crash at I/O call {n}/{total}"),
            quick,
        );
        std::fs::remove_file(&file).unwrap();
        points += 1;
    }
    eprintln!("{mode}: checked {points} crash points of {total} I/O calls");
}

// @tg: verifies:: [[openspec:transactions#Per-transaction durability levels#Sync commit survives a crash]] {level: crash}
// @tg: verifies:: [[openspec:transactions#Per-transaction durability levels#Ordered commit is acknowledged after its group is durable]] {level: crash}
// @tg: verifies:: [[openspec:transactions#Per-transaction durability levels#Relaxed commit may be lost but never corrupts]] {level: crash}
// @tg: verifies:: [[openspec:transactions#Atomic transactions#Crash during commit]] {level: crash}
// @tg: verifies:: [[21-e2e-tests#Crash at every commit point]] {level: crash}
#[test]
fn crash_at_every_io_call_process_kill() {
    if std::env::var(ENV_PATH).is_ok() {
        return;
    }
    sweep("kill");
}

#[test]
fn crash_at_every_io_call_power_cut() {
    if std::env::var(ENV_PATH).is_ok() {
        return;
    }
    sweep("power");
}

/// Rule `tx-quick-repair`: with quick repair on, every crash point still
/// recovers a durable prefix.
#[test]
fn crash_at_every_io_call_quick_repair() {
    if std::env::var(ENV_PATH).is_ok() {
        return;
    }
    sweep_with("kill", true);
}
