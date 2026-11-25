//! redb-specific experiments: crash-repair time (C), durability mapping (D), savepoints (C).

use crate::data::*;
use crate::engine::*;
use crate::redb_eng::v2::RedbEngine;
use crate::work::*;
use crate::Args;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use redb::{Builder, Database, Durability, ReadableTable, TableDefinition};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn meta_path(db: &Path) -> PathBuf {
    db.with_extension("meta.json")
}

fn load_meta(db: &Path) -> DsMeta {
    serde_json::from_slice(&std::fs::read(meta_path(db)).unwrap()).unwrap()
}

fn exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

pub fn main(cmd: &str, a: &Args, seed: u64) {
    match cmd {
        "crash-prep" => prep(a, seed),
        "crash-child" => child(a),
        "open-timer" => open_timer(a),
        "crash-run" => run(a, seed),
        "dur-child" => dur_child(a),
        "dur-test" => dur_test(a, seed),
        "savepoint-test" => savepoint_test(a, seed),
        "crash-reqbytes" => reqbytes(a, seed),
        _ => panic!("unknown {}", cmd),
    }
}

/// Build a clean redb file of N statements (3 orderings) and close it.
fn prep(a: &Args, seed: u64) {
    let n = a.u("n", 10_000_000);
    let db = PathBuf::from(a.s("db", &format!("data/crash/base-{}.redb", n)));
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&db);
    let cache = a.u("cache_mb", 1024) as usize * 1024 * 1024;
    let e = RedbEngine::open(&db, cache, &Tab::ALL);
    let t0 = Instant::now();
    let meta = bulk_load(&e, n, &[Tab::Spo, Tab::Pos, Tab::Osp], seed, a.u("batch", 250_000), true);
    let load_s = t0.elapsed().as_secs_f64();
    drop(e);
    std::fs::write(meta_path(&db), serde_json::to_vec(&meta).unwrap()).unwrap();
    let (len, alloc) = file_sizes(&[db.clone()]);
    let out = json!({"experiment": "C prep", "env": crate::env_info(seed), "n": n, "load_s": load_s, "file_len": len, "alloc": alloc});
    println!("{}", out);
    std::fs::create_dir_all("results").unwrap();
    std::fs::write(format!("results/C-prep-{}.json", n), serde_json::to_string_pretty(&out).unwrap()).unwrap();
}

fn small_txns(e: &RedbEngine, ds: &Ds, rng: &mut SmallRng, k: u64, d: Dur, last: Dur) -> Value {
    let mut c = OpCtx::new();
    let mut h = new_hist();
    for i in 0..k {
        let dd = if i + 1 == k { last } else { d };
        let t = Instant::now();
        run_op(e, ds, Op::Insert, dd, rng, &mut c);
        h.record((t.elapsed().as_nanos() as u64).max(1)).unwrap();
    }
    hist_summary(&h)
}

/// Child: open the db, put it into the requested state, print READY <json>, then sleep
/// until killed (or exit for `clean`).
fn child(a: &Args) {
    let db = PathBuf::from(a.s("db", ""));
    let mode = a.s("mode", "");
    let meta = load_meta(&db);
    let ds = Ds::from_meta(&meta);
    let mut rng = SmallRng::seed_from_u64(a.u("seed", 1));
    let t0 = Instant::now();
    let mut e = RedbEngine::open(&db, 1 << 30, &Tab::ALL);
    let open_s = t0.elapsed().as_secs_f64();
    let mut info = json!({"child_open_s": open_s});
    let qr = mode.starts_with("qr_");
    e.quick_repair = qr;
    match mode.trim_start_matches("qr_") {
        "clean" => {
            info["small"] = small_txns(&e, &ds, &mut rng, 50, Dur::NoFlush, Dur::Flush);
            drop(e);
            println!("READY {}", info);
            return;
        }
        "uncommitted_big" => {
            if qr {
                info["small"] = small_txns(&e, &ds, &mut rng, 10, Dur::Flush, Dur::Flush);
            }
            let m = std::env::var("BAKEOFF_BIG").ok().map(|v| v.parse().unwrap()).unwrap_or(a.u("big", 1_000_000));
            let mut w = e.begin_write();
            for _ in 0..m {
                let k = ds.next_stmt.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let id = stmt_id(k);
                let (s, p, o) = (ds.rnode(&mut rng), rand_pred(&mut rng), rand_obj(&mut rng, ds.nodes));
                for t in [Tab::Spo, Tab::Pos, Tab::Osp] {
                    w.insert(t, &t.key(s, p, o), &id.to_le_bytes());
                }
                w.insert(Tab::Stmt, &id.to_be_bytes(), &stmt_val(s, p, o, 0));
            }
            info["uncommitted_stmts"] = json!(m);
            println!("READY {}", info);
            std::mem::forget(w);
        }
        "after_noflush" => {
            if qr {
                info["flush_txns"] = small_txns(&e, &ds, &mut rng, 200, Dur::Flush, Dur::Flush);
            }
            info["noflush_txns"] = small_txns(&e, &ds, &mut rng, 200, Dur::NoFlush, Dur::NoFlush);
            println!("READY {}", info);
        }
        "after_flush" => {
            info["noflush_txns"] = small_txns(&e, &ds, &mut rng, 200, Dur::NoFlush, Dur::NoFlush);
            info["flush_txns"] = small_txns(&e, &ds, &mut rng, 200, Dur::Flush, Dur::Flush);
            println!("READY {}", info);
        }
        m => panic!("mode {}", m),
    }
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    std::mem::forget(e);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// Fresh process: time Database::open (including any repair) and a first query.
fn open_timer(a: &Args) {
    let db = PathBuf::from(a.s("db", ""));
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let c2 = calls.clone();
    let t0 = Instant::now();
    let d = Builder::new()
        .set_cache_size(1 << 30)
        .set_repair_callback(move |_s| {
            c2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })
        .open(&db)
        .expect("open");
    let open_s = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    const STMT: TableDefinition<&[u8; 8], &[u8; 26]> = TableDefinition::new("stmt");
    let r = d.begin_read().unwrap();
    let t = r.open_table(STMT).unwrap();
    let found = t.get(&stmt_id(12345).to_be_bytes()).unwrap().is_some();
    let q_s = t1.elapsed().as_secs_f64();
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    println!(
        "{}",
        json!({"open_s": open_s, "repair_callback_calls": calls.load(std::sync::atomic::Ordering::Relaxed),
            "first_query_s": q_s, "first_query_found": found, "max_rss_bytes": ru.ru_maxrss,
            "ru_inblock": ru.ru_inblock})
    );
}

fn spawn_child(db: &Path, mode: &str, seed: u64) -> (Value, f64) {
    let mut ch = Command::new(exe())
        .args(["crash-child", "--db", db.to_str().unwrap(), "--mode", mode, "--seed", &seed.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut rd = BufReader::new(ch.stdout.take().unwrap());
    let mut line = String::new();
    rd.read_line(&mut line).unwrap();
    assert!(line.starts_with("READY"), "child said {:?}", line);
    let info: Value = serde_json::from_str(line.trim_start_matches("READY ").trim()).unwrap();
    let t = Instant::now();
    if mode.ends_with("clean") {
        ch.wait().unwrap();
    } else {
        unsafe { libc::kill(ch.id() as i32, libc::SIGKILL) };
        ch.wait().unwrap();
    }
    (info, t.elapsed().as_secs_f64())
}

fn time_open(db: &Path) -> Value {
    let o = Command::new(exe()).args(["open-timer", "--db", db.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "open-timer failed: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

/// For each mode and repetition: APFS clone of the base file (cp -c), child, kill -9, timed open.
fn run(a: &Args, seed: u64) {
    let base = PathBuf::from(a.s("base", ""));
    let modes = a.s("modes", "clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush,qr_uncommitted_big");
    let reps = a.u("reps", 3);
    let mut out = Vec::new();
    for mode in modes.split(',') {
        for r in 0..reps {
            let trial = base.with_file_name(format!("trial-{}.redb", mode));
            let _ = std::fs::remove_file(&trial);
            let st = Command::new("cp").args(["-c", base.to_str().unwrap(), trial.to_str().unwrap()]).status().unwrap();
            assert!(st.success());
            std::fs::copy(meta_path(&base), meta_path(&trial)).unwrap();
            eprintln!("crash-run {} rep {}", mode, r);
            let (info, _) = spawn_child(&trial, mode, seed + r);
            let o = time_open(&trial);
            // second open after the repaired one closed cleanly
            let o2 = time_open(&trial);
            eprintln!("  open {:.3}s (then clean reopen {:.3}s)", o["open_s"].as_f64().unwrap(), o2["open_s"].as_f64().unwrap());
            out.push(json!({"mode": mode, "rep": r, "child": info, "open_after": o, "reopen_after_clean_close": o2}));
            let _ = std::fs::remove_file(&trial);
            let _ = std::fs::remove_file(meta_path(&trial));
        }
    }
    let (len, alloc) = file_sizes(&[base.clone()]);
    let res = json!({"experiment": "C crash repair", "env": crate::env_info(seed), "base": base, "base_file_len": len,
        "base_alloc": alloc, "meta": load_meta(&base), "trials": out});
    let path = a.s("out", "results/C-crash.json");
    std::fs::write(&path, serde_json::to_string_pretty(&res).unwrap()).unwrap();
    eprintln!("wrote {}", path);
}

// ------------------------------------------------------------------ D: durability mapping

const KV: TableDefinition<&str, u64> = TableDefinition::new("kv");

fn put(db: &Database, k: &str, d: Durability) {
    let mut w = db.begin_write().unwrap();
    w.set_durability(d);
    {
        let mut t = w.open_table(KV).unwrap();
        t.insert(k, 1).unwrap();
    }
    w.commit().unwrap();
}

fn dur_child(a: &Args) {
    let db = Database::create(a.s("db", "")).unwrap();
    put(&db, "base", Durability::Immediate);
    match a.s("scenario", "").as_str() {
        "none_imm_none" => {
            put(&db, "A_none", Durability::None);
            put(&db, "B_immediate", Durability::Immediate);
            put(&db, "C_none", Durability::None);
        }
        "none_only" => put(&db, "A_none", Durability::None),
        "eventual_only" => put(&db, "A_eventual", Durability::Eventual),
        "none_many_then_imm" => {
            for i in 0..100 {
                put(&db, &format!("N{}", i), Durability::None);
            }
            put(&db, "B_immediate", Durability::Immediate);
        }
        s => panic!("{}", s),
    }
    println!("READY");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    std::mem::forget(db);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn dur_test(a: &Args, seed: u64) {
    let dir = PathBuf::from("data/dur");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = Vec::new();
    for sc in ["none_imm_none", "none_only", "eventual_only", "none_many_then_imm"] {
        for r in 0..a.u("reps", 3) {
            let db = dir.join(format!("{}.redb", sc));
            let _ = std::fs::remove_file(&db);
            let mut ch = Command::new(exe())
                .args(["dur-child", "--db", db.to_str().unwrap(), "--scenario", sc])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            BufReader::new(ch.stdout.take().unwrap()).read_line(&mut line).unwrap();
            assert!(line.starts_with("READY"));
            unsafe { libc::kill(ch.id() as i32, libc::SIGKILL) };
            ch.wait().unwrap();
            let t = Instant::now();
            let d = Database::open(&db).unwrap();
            let open_s = t.elapsed().as_secs_f64();
            let rt = d.begin_read().unwrap();
            let tb = rt.open_table(KV).unwrap();
            let mut present = Vec::new();
            for e in tb.iter().unwrap() {
                present.push(e.unwrap().0.value().to_string());
            }
            present.sort();
            let np = present.len();
            let shown: Vec<_> = if np > 8 { vec![format!("{} keys incl. {:?}", np, present.iter().filter(|k| !k.starts_with('N')).collect::<Vec<_>>())] } else { present.clone() };
            eprintln!("{} rep {}: present after kill -9 = {:?}", sc, r, shown);
            out.push(json!({"scenario": sc, "rep": r, "present": present, "open_s": open_s}));
            drop(tb);
            drop(rt);
            drop(d);
            let _ = std::fs::remove_file(&db);
        }
    }
    let res = json!({"experiment": "D durability mapping (process kill -9; not power loss)", "env": crate::env_info(seed), "trials": out});
    std::fs::write("results/D-durability.json", serde_json::to_string_pretty(&res).unwrap()).unwrap();
}

// ------------------------------------------------------------------ savepoints

fn savepoint_test(_a: &Args, seed: u64) {
    let dir = PathBuf::from("data/sp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sp.redb");
    let _ = std::fs::remove_file(&path);
    let db = Database::create(&path).unwrap();
    put(&db, "base", Durability::Immediate);
    let mut res = serde_json::Map::new();
    let fmt = |r: Result<String, redb::SavepointError>| -> Value {
        match r {
            Ok(s) => json!({"ok": s}),
            Err(e) => json!({"err_display": e.to_string(), "err_debug": format!("{:?}", e)}),
        }
    };
    // 1. clean txn
    {
        let w = db.begin_write().unwrap();
        res.insert("ephemeral_on_clean_txn".into(), fmt(w.ephemeral_savepoint().map(|_| "Savepoint".into())));
        w.abort().unwrap();
    }
    // 2. after open_table only (no write)
    {
        let w = db.begin_write().unwrap();
        {
            let _t = w.open_table(KV).unwrap();
        }
        res.insert("ephemeral_after_open_table_no_write".into(), fmt(w.ephemeral_savepoint().map(|_| "Savepoint".into())));
        w.abort().unwrap();
    }
    // 3. after a write
    {
        let w = db.begin_write().unwrap();
        {
            let mut t = w.open_table(KV).unwrap();
            t.insert("x", 1).unwrap();
        }
        res.insert("ephemeral_after_write".into(), fmt(w.ephemeral_savepoint().map(|_| "Savepoint".into())));
        res.insert("persistent_after_write".into(), fmt(w.persistent_savepoint().map(|id| format!("id {}", id))));
        w.abort().unwrap();
    }
    // 4. persistent on clean txn with Durability::None
    {
        let mut w = db.begin_write().unwrap();
        w.set_durability(Durability::None);
        res.insert("persistent_on_clean_txn_durability_none".into(), fmt(w.persistent_savepoint().map(|id| format!("id {}", id))));
        w.abort().unwrap();
    }
    // 5. savepoint first, then write, then restore inside the same txn (per-request rollback
    //    of the first request only) and a second savepoint after the restore
    {
        let mut w = db.begin_write().unwrap();
        let sp = w.ephemeral_savepoint().unwrap();
        {
            let mut t = w.open_table(KV).unwrap();
            t.insert("req1", 1).unwrap();
        }
        let r = w.restore_savepoint(&sp);
        res.insert("restore_savepoint_after_write_same_txn".into(), match r {
            Ok(()) => json!({"ok": "restored"}),
            Err(e) => json!({"err_display": e.to_string(), "err_debug": format!("{:?}", e)}),
        });
        let present = {
            let t = w.open_table(KV).unwrap();
            let x = t.get("req1").unwrap().is_some();
            x
        };
        res.insert("req1_visible_after_restore".into(), json!(present));
        res.insert("ephemeral_after_restore".into(), fmt(w.ephemeral_savepoint().map(|_| "Savepoint".into())));
        w.abort().unwrap();
    }
    let out = json!({"experiment": "savepoint after write (redb 2.6.4)", "env": crate::env_info(seed), "result": res});
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    std::fs::write("results/C-savepoint.json", serde_json::to_string_pretty(&out).unwrap()).unwrap();
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ bytes per request (ADR 0025)

/// On an APFS clone of a loaded redb file: disk bytes written per one-statement request with
/// (a) one Immediate engine txn per request, (b) one Immediate txn per batch of 100 requests,
/// (c) one None txn per request with an Immediate every 100.
fn reqbytes(a: &Args, seed: u64) {
    let base = PathBuf::from(a.s("base", ""));
    let trial = base.with_file_name("trial-reqbytes.redb");
    let _ = std::fs::remove_file(&trial);
    assert!(Command::new("cp").args(["-c", base.to_str().unwrap(), trial.to_str().unwrap()]).status().unwrap().success());
    let meta = load_meta(&base);
    let ds = Ds::from_meta(&meta);
    let e = RedbEngine::open(&trial, 1 << 30, &Tab::ALL);
    let mut rng = SmallRng::seed_from_u64(seed);
    let n = a.u("requests", 2000);
    let one = |w: &mut crate::redb_eng::v2::W, rng: &mut SmallRng| {
        let k = ds.next_stmt.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = stmt_id(k);
        let (s, p, o) = (ds.rnode(rng), rand_pred(rng), rand_obj(rng, ds.nodes));
        for t in [Tab::Spo, Tab::Pos, Tab::Osp] {
            w.insert(t, &t.key(s, p, o), &id.to_le_bytes());
        }
        w.insert(Tab::Stmt, &id.to_be_bytes(), &stmt_val(s, p, o, 0));
    };
    let mut out = Vec::new();
    for (label, per_txn, flush_every_txn) in [("txn_per_request_immediate", 1u64, 1u64), ("batch_100_immediate", 100, 1), ("txn_per_request_none_immediate_every_100", 1, 100)] {
        // warm-up
        for _ in 0..50 {
            let mut w = e.begin_write();
            one(&mut w, &mut rng);
            w.commit(Dur::Flush);
        }
        let (d0, _) = disk_written();
        let t0 = Instant::now();
        let txns = n / per_txn;
        for i in 0..txns {
            let mut w = e.begin_write();
            for _ in 0..per_txn {
                one(&mut w, &mut rng);
            }
            let d = if (i + 1) % flush_every_txn == 0 || i + 1 == txns { Dur::Flush } else { Dur::NoFlush };
            w.commit(d);
        }
        let wall = t0.elapsed().as_secs_f64();
        let (d1, _) = disk_written();
        eprintln!("{}: {:.0} bytes/request", label, (d1 - d0) as f64 / n as f64);
        out.push(json!({"mode": label, "requests": n, "statements_per_request": 1, "requests_per_txn": per_txn,
            "durable_every_txns": flush_every_txn, "disk_bytes_written": d1 - d0,
            "disk_bytes_per_request": (d1 - d0) as f64 / n as f64, "wall_s": wall, "requests_per_s": n as f64 / wall}));
    }
    drop(e);
    let _ = std::fs::remove_file(&trial);
    let res = json!({"experiment": "bytes written per one-statement request (ADR 0025)", "env": crate::env_info(seed),
        "base": base, "loaded_stmts": meta.loaded_stmts, "result": out});
    std::fs::write(a.s("out", "results/C-reqbytes-10M.json"), serde_json::to_string_pretty(&res).unwrap()).unwrap();
}
