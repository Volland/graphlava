mod crash;
mod data;
mod engine;
mod mdbx_eng;
mod redb_eng;
mod sana_eng;
mod work;

use data::*;
use engine::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use work::*;

pub struct Args(HashMap<String, String>);
impl Args {
    fn parse() -> (String, Args) {
        let a: Vec<String> = std::env::args().collect();
        let cmd = a.get(1).cloned().unwrap_or_default();
        let mut m = HashMap::new();
        let mut i = 2;
        while i < a.len() {
            let k = a[i].trim_start_matches("--").to_string();
            let v = a.get(i + 1).cloned().unwrap_or_default();
            m.insert(k, v);
            i += 2;
        }
        (cmd, Args(m))
    }
    pub fn s(&self, k: &str, d: &str) -> String {
        self.0.get(k).cloned().unwrap_or_else(|| d.to_string())
    }
    pub fn u(&self, k: &str, d: u64) -> u64 {
        self.0.get(k).map(|v| v.replace('_', "").parse().unwrap()).unwrap_or(d)
    }
}

fn sh(cmd: &str) -> String {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn env_info(seed: u64) -> Value {
    json!({
        "cpu": sh("sysctl -n machdep.cpu.brand_string"),
        "cores": sh("sysctl -n hw.ncpu"),
        "ram_bytes": sh("sysctl -n hw.memsize"),
        "os": sh("sw_vers -productName") + " " + &sh("sw_vers -productVersion") + " (" + &sh("sw_vers -buildVersion") + ")",
        "kernel": sh("uname -r"),
        "fs": sh("mount | grep ' on /System/Volumes/Data '"),
        "rustc": sh("rustc --version"),
        "build": "cargo --release, lto=fat, codegen-units=1, debug=1 (symbols only)",
        "graphlava_commit": sh("git -C ../.. rev-parse --short HEAD") + " (+ uncommitted working tree)",
        "dataset": "gen-v1 (src/data.rs)",
        "seed": seed,
        "cache_state": "warm (OS page cache not dropped; `purge` needs root)",
        "timestamp": sh("date -u +%Y-%m-%dT%H:%M:%SZ"),
    })
}

fn rm_db(files: &[PathBuf], dir: &Path) {
    for f in files {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn sizes_json<E: Engine>(e: &E, live_stmts: u64) -> Value {
    let (len, alloc) = file_sizes(&e.files());
    json!({"file_len": len, "alloc_bytes": alloc, "live_stmts_approx": live_stmts,
        "bytes_per_stmt_len": len as f64 / live_stmts as f64, "bytes_per_stmt_alloc": alloc as f64 / live_stmts as f64})
}

fn write_json(path: &str, v: &Value) {
    std::fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(v).unwrap()).unwrap();
    eprintln!("wrote {}", path);
}

pub fn with_engine<F: EngineFn>(name: &str, dir: &Path, tables: &[Tab], cache: usize, f: F) -> Value {
    std::fs::create_dir_all(dir).unwrap();
    match name {
        "redb" => f.call(redb_eng::v2::RedbEngine::open(&dir.join("db.redb"), cache, tables)),
        "redb4" => f.call(redb_eng::v4::RedbEngine::open(&dir.join("db.redb"), cache, tables)),
        "mdbx" => f.call(mdbx_eng::MdbxEngine::open(&dir.join("mdbx"), tables)),
        "sana" => f.call(sana_eng::SanaEngine::open(&dir.join("db.sana"), tables)),
        _ => panic!("unknown engine {}", name),
    }
}
pub trait EngineFn {
    fn call<E: Engine>(self, e: E) -> Value;
}

// ------------------------------------------------------------------- bake (run A)

struct Bake<'a> {
    a: &'a Args,
    dir: PathBuf,
}
fn save_partial(a: &Args, res: &serde_json::Map<String, Value>) {
    let eng = a.s("engine", "redb");
    let n = a.u("n", 10_000_000);
    let p = format!("{}.partial", a.s("out", &format!("results/A-bake-{}-{}.json", eng, n)));
    let _ = std::fs::write(p, serde_json::to_string(&json!({"partial": true, "result": res})).unwrap());
}
impl<'a> EngineFn for Bake<'a> {
    fn call<E: Engine>(self, e: E) -> Value {
        let a = self.a;
        let n = a.u("n", 10_000_000);
        let seed = a.u("seed", 42);
        let runs = a.u("runs", 5);
        let ords = vec![Tab::Spo, Tab::Pos, Tab::Osp];
        let mut res = serde_json::Map::new();
        res.insert("engine".into(), json!(e.name()));
        res.insert("engine_info".into(), e.info());
        res.insert("n".into(), json!(n));
        eprintln!("[{}] bulk load {}", e.name(), n);
        let (dw0, _) = disk_written();
        let t0 = Instant::now();
        let meta = bulk_load(&e, n, &ords, seed, a.u("batch", 250_000), true);
        let load_s = t0.elapsed().as_secs_f64();
        let (dw1, _) = disk_written();
        res.insert("bulk_load".into(), json!({"wall_s": load_s, "stmts": meta.loaded_stmts, "nodes": meta.nodes,
            "stmts_per_s": meta.loaded_stmts as f64 / load_s, "disk_bytes_written": dw1 - dw0,
            "batch": a.u("batch", 250_000), "runs": 1}));
        res.insert("size_after_load".into(), sizes_json(&e, meta.loaded_stmts));
        let ds = Ds::from_meta(&meta);
        let phases = a.s("phases", "lat,tp,ow,probe");

        for (label, d, nops, warm, nr) in [
            ("latency_relaxed", Dur::NoFlush, a.u("relaxed_ops", 200_000), 20_000u64, runs),
            ("latency_sync", Dur::Flush, a.u("sync_ops", 20_000), 1_000u64, a.u("sync_runs", runs)),
        ] {
            let want = phases.split(',').any(|p| p == "lat" || (p == "latr" && d == Dur::NoFlush) || (p == "lats" && d == Dur::Flush));
            if !want {
                continue;
            }
            let mut v = Vec::new();
            for r in 0..nr {
                eprintln!("[{}] {} run {}", e.name(), label, r);
                v.push(latency_run(&e, &ds, &MIX, d, warm, nops, seed + 100 + r));
            }
            res.insert(label.into(), json!(v));
            save_partial(a, &res);
        }
        let mut tp = Vec::new();
        for d in [Dur::NoFlush, Dur::Flush] {
            if !phases.contains("tp") {
                break;
            }
            for readers in [1usize, 4, 16] {
                eprintln!("[{}] throughput {:?} {} readers", e.name(), d, readers);
                tp.push(throughput_run(&e, &ds, d, readers, a.u("tp_secs", 10) as f64, seed + 7));
            }
        }
        res.insert("throughput".into(), json!(tp));
        save_partial(a, &res);
        let live = ds.next_stmt.load(std::sync::atomic::Ordering::Relaxed);
        res.insert("size_before_overwrites".into(), sizes_json(&e, live));
        let now = if phases.contains("ow") { a.u("overwrites", 1_000_000) } else { 0 };
        eprintln!("[{}] {} overwrites", e.name(), now);
        res.insert("overwrite_phase".into(), overwrite_phase(&e, &ds, now, a.u("flush_every", 100), seed + 9));
        res.insert("size_after_overwrites".into(), sizes_json(&e, live));
        // NoFlush-only growth probe
        let ng = if phases.contains("probe") { a.u("noflush_probe", 100_000) } else { 0 };
        if ng > 0 {
            eprintln!("[{}] noflush-only probe {}", e.name(), ng);
            let before = sizes_json(&e, live);
            let mut rng = <rand::rngs::SmallRng as rand::SeedableRng>::seed_from_u64(seed + 11);
            let mut c = OpCtx::new();
            let t = Instant::now();
            for i in 0..ng {
                let op = if i % 2 == 0 { Op::PropOverwrite } else { Op::ObjOverwrite };
                run_op(&e, &ds, op, Dur::NoFlush, &mut rng, &mut c);
            }
            let wall = t.elapsed().as_secs_f64();
            let after = sizes_json(&e, live);
            let w = e.begin_write();
            w.commit(Dur::Flush);
            // a few flushed commits so freed pages can be recycled
            for _ in 0..10 {
                run_op(&e, &ds, Op::PropOverwrite, Dur::Flush, &mut rng, &mut c);
            }
            let after_flush = sizes_json(&e, live);
            res.insert("noflush_only_probe".into(), json!({"overwrites": ng, "wall_s": wall, "before": before,
                "after_noflush": after, "after_one_flush_plus_10": after_flush}));
        }
        let files = e.files();
        drop(e);
        rm_db(&files, &self.dir);
        Value::Object(res)
    }
}

// ------------------------------------------------------------------- orderings (run B)

struct Orders<'a> {
    a: &'a Args,
    dir: PathBuf,
    layout: usize,
}
impl<'a> EngineFn for Orders<'a> {
    fn call<E: Engine>(self, e: E) -> Value {
        let a = self.a;
        let n = a.u("n", 10_000_000);
        let seed = a.u("seed", 42);
        let runs = a.u("runs", 5);
        let ords: Vec<Tab> = if self.layout == 3 { vec![Tab::Spo, Tab::Pos, Tab::Osp] } else { Tab::ORD.to_vec() };
        let mut res = serde_json::Map::new();
        res.insert("engine".into(), json!(e.name()));
        res.insert("layout".into(), json!(ords.iter().map(|t| t.name()).collect::<Vec<_>>()));
        eprintln!("[{}] layout {} load", e.name(), self.layout);
        let t0 = Instant::now();
        let meta = bulk_load(&e, n, &ords, seed, a.u("batch", 250_000), true);
        res.insert("bulk_load_s".into(), json!(t0.elapsed().as_secs_f64()));
        res.insert("size_after_load".into(), sizes_json(&e, meta.loaded_stmts));
        let ds = Ds::from_meta(&meta);
        let wmix = [(Op::Insert, 50u32), (Op::ObjOverwrite, 50u32)];
        let skip_writes = a.u("skip_writes", 0) == 1;
        for (label, d, nops, nr) in [
            ("write_relaxed", Dur::NoFlush, a.u("relaxed_ops", 50_000), runs),
            ("write_sync", Dur::Flush, a.u("sync_ops", 3_000), a.u("sync_runs", 3)),
        ] {
            if skip_writes {
                continue;
            }
            let mut v = Vec::new();
            for r in 0..nr {
                eprintln!("[{}] layout {} {} run {}", e.name(), self.layout, label, r);
                v.push(latency_run(&e, &ds, &wmix, d, 2_000, nops, seed + 200 + r));
            }
            res.insert(label.into(), json!(v));
        }
        let nq = a.u("nq", 2_000);
        let mut cyc = Vec::new();
        let plans: Vec<Plan> = if self.layout == 3 { vec![Plan::Three] } else { vec![Plan::Three, Plan::Six] };
        for plan in plans {
            for r in 0..runs {
                eprintln!("[{}] layout {} cyclic {:?} run {}", e.name(), self.layout, plan, r);
                let mut v = cyclic_suite(&e, &ds, plan, nq, seed + 300 + r);
                v["layout"] = json!(self.layout);
                cyc.push(v);
            }
        }
        if self.layout == 3 {
            let mut builds = Vec::new();
            for t in [Tab::Pso, Tab::Sop, Tab::Ops] {
                eprintln!("[{}] online build {}", e.name(), t.name());
                let bd = if a.s("build_dur", "flush") == "flush" { Dur::Flush } else { Dur::NoFlush };
                builds.push(build_ordering_d(&e, t, a.u("build_batch", 100_000) as usize, bd));
            }
            res.insert("online_builds".into(), json!(builds));
            res.insert("size_after_builds".into(), sizes_json(&e, meta.loaded_stmts));
            for r in 0..runs {
                eprintln!("[{}] layout 3+ondemand cyclic Six run {}", e.name(), r);
                let mut v = cyclic_suite(&e, &ds, Plan::Six, nq, seed + 300 + r);
                v["layout"] = json!("3+ondemand(pso,sop,ops)");
                cyc.push(v);
            }
        }
        res.insert("cyclic".into(), json!(cyc));
        let files = e.files();
        drop(e);
        rm_db(&files, &self.dir);
        Value::Object(res)
    }
}

fn main() {
    let (cmd, a) = Args::parse();
    let seed = a.u("seed", 42);
    let root = PathBuf::from(a.s("data", "data"));
    let cache = a.u("cache_mb", 1024) as usize * 1024 * 1024;
    work::CHECKPOINT.store(a.u("checkpoint", 100), std::sync::atomic::Ordering::Relaxed);
    mdbx_eng::MDBX_MODE.store(a.u("mdbx_mode", 0) as u8, std::sync::atomic::Ordering::Relaxed);
    match cmd.as_str() {
        "bake" => {
            let eng = a.s("engine", "redb");
            let n = a.u("n", 10_000_000);
            let dir = root.join(format!("bake-{}-{}", eng, n));
            let _ = std::fs::remove_dir_all(&dir);
            let r = with_engine(&eng, &dir, &Tab::ALL, cache, Bake { a: &a, dir: dir.clone() });
            let out = json!({"experiment": "A engine bake-off", "env": env_info(seed), "args": a.0, "mix": MIX.iter().map(|(o, w)| json!({"op": o.name(), "pct": w})).collect::<Vec<_>>(), "result": r});
            write_json(&a.s("out", &format!("results/A-bake-{}-{}.json", eng, n)), &out);
        }
        "orders" => {
            let eng = a.s("engine", "redb");
            let n = a.u("n", 10_000_000);
            let layout = a.u("layout", 3) as usize;
            let dir = root.join(format!("orders-{}-{}-{}", eng, layout, n));
            let _ = std::fs::remove_dir_all(&dir);
            let r = with_engine(&eng, &dir, &Tab::ALL, cache, Orders { a: &a, dir: dir.clone(), layout });
            let out = json!({"experiment": "B ordering count", "env": env_info(seed), "args": a.0, "result": r});
            write_json(&a.s("out", &format!("results/B-orders-{}-{}-{}.json", eng, layout, n)), &out);
        }
        "tpcheck" => {
            // concurrency check: load n, then throughput with many readers, count reader errors
            crate::mdbx_eng::MDBX_MODE.store(a.u("mdbx_mode", 0) as u8, std::sync::atomic::Ordering::Relaxed);
            let eng = a.s("engine", "mdbx");
            let dir = root.join(format!("tpcheck-{}", eng));
            let _ = std::fs::remove_dir_all(&dir);
            let v = with_engine(&eng, &dir, &Tab::ALL, cache, TpCheck { a: &a, dir: dir.clone() });
            println!("{}", v);
        }
        "probe" => {
            // quick write-cost probe: load n, then k small writes with checkpoint every 100
            crate::mdbx_eng::MDBX_MODE.store(a.u("mdbx_mode", 0) as u8, std::sync::atomic::Ordering::Relaxed);
            let eng = a.s("engine", "mdbx");
            let dir = root.join(format!("probe-{}", eng));
            let _ = std::fs::remove_dir_all(&dir);
            let v = with_engine(&eng, &dir, &Tab::ALL, cache, Probe { a: &a, dir: dir.clone() });
            println!("{}", v);
        }
        "smoke" => {
            for eng in a.s("engines", "redb,redb4,mdbx,sana").split(",") {
                let dir = root.join(format!("smoke-{}", eng));
                let _ = std::fs::remove_dir_all(&dir);
                let v = with_engine(eng, &dir, &Tab::ALL, cache, Smoke { dir: dir.clone() });
                println!("{} {}", eng, v);
            }
        }
        c if c.starts_with("crash") || c.starts_with("dur") || c.starts_with("savepoint") || c == "open-timer" => {
            crash::main(c, &a, seed)
        }
        _ => eprintln!("commands: bake | orders | smoke | crash-* | dur-* | savepoint-test | open-timer"),
    }
}

struct Smoke {
    dir: PathBuf,
}
impl EngineFn for Smoke {
    fn call<E: Engine>(self, e: E) -> Value {
        let ords = vec![Tab::Spo, Tab::Pos, Tab::Osp];
        let meta = bulk_load(&e, 200_000, &ords, 1, 50_000, false);
        let ds = Ds::from_meta(&meta);
        // correctness checks
        let r = e.begin_read();
        let mut buf = Vec::new();
        assert!(r.get(Tab::Stmt, &stmt_id(5).to_be_bytes(), &mut buf));
        let (s, p, o) = stmt_decode(&buf);
        let mut b2 = Vec::new();
        assert!(r.get(Tab::Spo, &Tab::Spo.key(s, p, o), &mut b2));
        assert_eq!(u64::from_le_bytes(b2[..8].try_into().unwrap()), stmt_id(5));
        assert!(r.get(Tab::Osp, &Tab::Osp.key(s, p, o), &mut b2));
        let mut cnt = 0;
        let (lo, hi) = prefix_bounds(&s.to_be_bytes(), 24);
        r.scan(Tab::Spo, &lo, &hi, &mut |_, _| {
            cnt += 1;
            true
        });
        assert!(cnt >= 1);
        assert!(r.get(Tab::Props, &s.to_be_bytes(), &mut buf));
        drop(r);
        let l = latency_run(&e, &ds, &MIX, Dur::NoFlush, 100, 5_000, 3);
        let l2 = latency_run(&e, &ds, &MIX, Dur::Flush, 10, 50, 3);
        let tp = throughput_run(&e, &ds, Dur::NoFlush, 4, 1.0, 5);
        let ow = overwrite_phase(&e, &ds, 2_000, 100, 4);
        let c3 = cyclic_suite(&e, &ds, Plan::Three, 50, 1);
        for t in [Tab::Pso, Tab::Sop, Tab::Ops] {
            build_ordering(&e, t, 10_000);
        }
        let c6 = cyclic_suite(&e, &ds, Plan::Six, 50, 1);
        // the two plans must agree on results
        for q in ["tri1_anchored_p0", "cyc4_anchored_p0", "tri_anypred_anchored", "tri_all_rare_pred"] {
            assert_eq!(c3["queries"][q]["results"], c6["queries"][q]["results"], "plan mismatch on {}", q);
        }
        let sz = sizes_json(&e, meta.loaded_stmts);
        let files = e.files();
        drop(e);
        rm_db(&files, &self.dir);
        json!({"mix_p99": l["all"]["p99_us"], "sync_p50": l2["all"]["p50_us"], "tp": tp, "ow": ow["wall_s"],
            "c3": c3["queries"]["tri1_anchored_p0"]["results"], "c3any": c3["queries"]["tri_anypred_anchored"]["results"],
            "c3all": c3["queries"]["tri_all_rare_pred"]["results"], "size": sz})
    }
}

struct Probe<'a> {
    a: &'a Args,
    dir: PathBuf,
}
impl<'a> EngineFn for Probe<'a> {
    fn call<E: Engine>(self, e: E) -> Value {
        let n = self.a.u("n", 1_000_000);
        let meta = bulk_load(&e, n, &[Tab::Spo, Tab::Pos, Tab::Osp], 42, 250_000, false);
        let ds = Ds::from_meta(&meta);
        let d = if self.a.s("dur", "noflush") == "flush" { Dur::Flush } else { Dur::NoFlush };
        let wmix = [(Op::PropOverwrite, 50u32), (Op::ObjOverwrite, 50u32)];
        let l = latency_run(&e, &ds, &wmix, d, 200, self.a.u("k", 3000), 1);
        let info = e.info();
        let sz = sizes_json(&e, meta.loaded_stmts);
        let files = e.files();
        drop(e);
        rm_db(&files, &self.dir);
        json!({"info": info, "wall_s": l["wall_s"], "p50": l["all"]["p50_us"], "p99": l["all"]["p99_us"], "disk_B_per_write": l["disk_bytes_per_write_op"], "size": sz})
    }
}

struct TpCheck<'a> {
    a: &'a Args,
    dir: PathBuf,
}
impl<'a> EngineFn for TpCheck<'a> {
    fn call<E: Engine>(self, e: E) -> Value {
        let n = self.a.u("n", 1_000_000);
        let meta = bulk_load(&e, n, &[Tab::Spo, Tab::Pos, Tab::Osp], 42, 250_000, false);
        let ds = Ds::from_meta(&meta);
        let mut v = Vec::new();
        for _ in 0..self.a.u("reps", 3) {
            v.push(throughput_run(&e, &ds, Dur::NoFlush, self.a.u("readers", 16) as usize, self.a.u("secs", 10) as f64, 7));
        }
        let files = e.files();
        drop(e);
        rm_db(&files, &self.dir);
        json!(v)
    }
}
