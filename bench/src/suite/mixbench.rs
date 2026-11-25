//! The agent-memory mix on every adapter, plus the nightly sections built on
//! it: scaling sweep, reader scaling, open and recovery time, space under churn
//! ([[17-benchmarks#Scaling sweep]] and following).

use crate::adapter::{Adapter, Dur, Engine};
use crate::gen::{self, Shape};
use crate::mix::{self, DriveCfg, State};
use crate::run::{durability_matches, Ctx};
use crate::stats::{RunSeries, Hists};
use anyhow::Result;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, clap::Args)]
pub struct MixArgs {
    /// Node-valued statements in the dataset (N; properties add 4 per node).
    #[arg(long, default_value_t = 1_000_000)]
    pub statements: u64,
    /// Recorded operations per run (spec: at least 10^5).
    #[arg(long, default_value_t = 100_000)]
    pub ops: u64,
    /// Warm-up operations before each run (not recorded).
    #[arg(long, default_value_t = 10_000)]
    pub warmup: u64,
    /// Worker threads issuing operations.
    #[arg(long, default_value_t = 1)]
    pub threads: usize,
    /// Open-loop arrival rate in ops/s (latency mode). 0 = closed loop (throughput mode).
    #[arg(long, default_value_t = 2000.0)]
    pub rate: f64,
    /// Load batch size (statements per write transaction).
    #[arg(long, default_value_t = 5000)]
    pub load_batch: usize,
    /// Versions per fact before the runs (tiramemsu comparison: 1, 10, 100):
    /// every node property is overwritten `versions - 1` times.
    #[arg(long, default_value_t = 1)]
    pub versions: u64,
}

fn drive_cfg(c: &Ctx, m: &MixArgs, run: usize) -> DriveCfg {
    DriveCfg {
        ops: m.ops,
        warmup: m.warmup,
        threads: m.threads.max(1),
        rate: (m.rate > 0.0).then_some(m.rate),
        dur: c.c.durability,
        seed: c.c.seed.wrapping_add(1000 * run as u64 + 1),
        mix: mix::MIX.to_vec(),
    }
}

/// Overwrites every node property `versions - 1` times (history depth).
fn add_versions(a: &dyn Adapter, shape: &Shape, versions: u64, seed: u64) -> Result<()> {
    if versions <= 1 {
        return Ok(());
    }
    let mut s = a.session()?;
    let mut ops = Vec::new();
    let mut k = 0u64;
    for _ in 1..versions {
        gen::stream(shape, |g| {
            if let gen::Gen::Prop { s: n, p, v } = g {
                k += 1;
                ops.push(crate::adapter::WOp::Overwrite {
                    s: crate::adapter::Term::Iri(gen::node_iri(n)),
                    p: gen::lpred(p),
                    o: crate::adapter::Term::int(v ^ (k.wrapping_mul(seed) as i64 & 0xFFFF)),
                });
            }
        });
        for c in ops.chunks(5000) {
            s.write(c, Dur::Relaxed)?;
        }
        ops.clear();
    }
    drop(s);
    a.checkpoint()
}

/// Loads, then runs the mix `runs` times on one engine.
pub fn mix_one(c: &Ctx, e: Engine, m: &MixArgs, tag: &str) -> Result<Value> {
    let dir = c.fresh_dir(&format!("{}-mix-{tag}", e.name()))?;
    let mut a = Some(c.open(e, &dir)?);
    let shape = Shape::new(m.statements, c.c.seed);
    eprintln!("[{}] loading {} node statements ({:?})", e.name(), m.statements, shape);
    let loaded = gen::load(a.as_deref().expect("open"), &shape, m.load_batch)?;
    let load = loaded.load.clone();
    add_versions(a.as_deref().expect("open"), &shape, m.versions, c.c.seed)?;
    let st = State::new(loaded);
    let mut series = RunSeries::default();
    let mut runs = Vec::new();
    for run in 0..c.c.runs {
        let cache = c.prepare_run(e, &dir, &mut a)?;
        let ad = a.as_deref().expect("open");
        let out = mix::drive(ad, &st, &drive_cfg(c, m, run))?;
        series.push_hists(&out.latency);
        series.push("ops_per_s", out.ops as f64 / out.wall_s);
        series.push("disk_bytes", ad.disk_size() as f64);
        let mut j = out.to_json();
        j["run"] = json!(run);
        j["cache_state"] = json!(cache);
        eprintln!(
            "[{}] run {run}: {:.0} ops/s, all p99 {:.1} us, errors {:?}",
            e.name(),
            out.ops as f64 / out.wall_s,
            out.latency.all().value_at_quantile(0.99) as f64 / 1e3,
            out.counters.errors
        );
        runs.push(j);
    }
    let res = json!({
        "adapter": e.name(),
        "describe": a.as_ref().map(|a| a.describe()),
        "durability": {"requested": c.c.durability, "matches": durability_matches(e, c.c.durability)},
        "dataset": {"generator": gen::GEN_VERSION, "shape": shape, "versions_per_fact": m.versions},
        "mode": if m.rate > 0.0 { format!("open-loop {} ops/s", m.rate) } else { "closed-loop".into() },
        "load": load,
        "file_bytes_after_runs": a.as_ref().map(|a| a.disk_size()),
        "summary": series.to_json(),
        "runs": runs,
    });
    drop(a);
    c.cleanup(&dir);
    Ok(res)
}

pub fn mix(c: &Ctx, m: &MixArgs) -> Result<()> {
    let mut per = Vec::new();
    for &e in &c.c.adapters {
        per.push(mix_one(c, e, m, "main")?);
    }
    c.save(
        "agent-mix",
        serde_json::to_value(format!("{m:?}"))?,
        json!({"adapters": per}),
    )
}

// ---------------------------------------------------------------- scaling sweep

#[derive(Clone, Debug, clap::Args)]
pub struct ScalingArgs {
    /// Sizes in node statements (spec: 1e5..1e9 desktop).
    #[arg(long, value_delimiter = ',', default_value = "100000,1000000,10000000")]
    pub sizes: Vec<u64>,
    #[command(flatten)]
    pub mix: MixArgs,
}

/// Least-squares slope of y against ln(x).
fn log_slope(xs: &[f64], ys: &[f64]) -> f64 {
    let lx: Vec<f64> = xs.iter().map(|x| x.ln()).collect();
    let n = lx.len() as f64;
    let mx = lx.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let num: f64 = lx.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = lx.iter().map(|x| (x - mx) * (x - mx)).sum();
    if den == 0.0 { 0.0 } else { num / den }
}

pub fn scaling(c: &Ctx, s: &ScalingArgs) -> Result<()> {
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let mut points = Vec::new();
        let (mut xs, mut lk, mut ow) = (Vec::new(), Vec::new(), Vec::new());
        for &n in &s.sizes {
            let mut m = s.mix.clone();
            m.statements = n;
            let r = mix_one(c, e, &m, &format!("scale{n}"))?;
            let med = |k: &str| r["summary"][k]["median"].as_f64().unwrap_or(f64::NAN);
            xs.push(n as f64);
            lk.push(med("props_get.p50_us"));
            ow.push(med("write_txn.p50_us"));
            points.push(json!({
                "statements": n,
                "props_get_p50_us": med("props_get.p50_us"), "props_get_p99_us": med("props_get.p99_us"),
                "stmt_get_p50_us": med("stmt_get.p50_us"), "stmt_get_p99_us": med("stmt_get.p99_us"),
                "write_txn_p50_us": med("write_txn.p50_us"), "write_txn_p99_us": med("write_txn.p99_us"),
                "bytes_per_statement": r["load"]["bytes_per_statement"],
                "full": r,
            }));
        }
        out.push(json!({
            "adapter": e.name(),
            "points": points,
            "slope_props_get_p50_us_per_ln_n": log_slope(&xs, &lk),
            "slope_write_txn_p50_us_per_ln_n": log_slope(&xs, &ow),
        }));
    }
    c.save("scaling", json!({"sizes": s.sizes, "mix": format!("{:?}", s.mix)}), json!({"adapters": out}))
}

// ---------------------------------------------------------------- reader scaling

#[derive(Clone, Debug, clap::Args)]
pub struct ReadersArgs {
    #[arg(long, default_value_t = 1_000_000)]
    pub statements: u64,
    /// Reader thread counts.
    #[arg(long, value_delimiter = ',', default_value = "0,1,2,4,8,16,32,64")]
    pub readers: Vec<usize>,
    /// Writer arrival rate (transactions/s, open loop at `--durability`).
    #[arg(long, default_value_t = 5000.0)]
    pub writer_rate: f64,
    /// Seconds per configuration.
    #[arg(long, default_value_t = 10.0)]
    pub secs: f64,
}

pub fn readers(c: &Ctx, r: &ReadersArgs) -> Result<()> {
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let dir = c.fresh_dir(&format!("{}-readers", e.name()))?;
        let a = c.open(e, &dir)?;
        let shape = Shape::new(r.statements, c.c.seed);
        let loaded = gen::load(a.as_ref(), &shape, 5000)?;
        let st = State::new(loaded);
        let mut points = Vec::new();
        for &nr in &r.readers {
            let mut series = RunSeries::default();
            let mut runs = Vec::new();
            for run in 0..c.c.runs {
                let size0 = a.disk_size();
                let wops = (r.writer_rate * r.secs) as u64;
                let wcfg = DriveCfg {
                    ops: wops,
                    warmup: 0,
                    threads: 1,
                    rate: Some(r.writer_rate),
                    dur: c.c.durability,
                    seed: c.c.seed + run as u64,
                    mix: mix::write_mix(),
                };
                let stop = std::sync::atomic::AtomicBool::new(false);
                let (w, rd) = std::thread::scope(|sc| -> Result<_> {
                    let readers: Vec<_> = (0..nr)
                        .map(|i| {
                            let (st, a, stop) = (&st, a.as_ref(), &stop);
                            sc.spawn(move || -> Result<(Hists, u64)> {
                                let mut s = a.session()?;
                                let mut rng = <rand::rngs::SmallRng as rand::SeedableRng>::seed_from_u64(c.c.seed ^ (i as u64 + 77));
                                let rmix = mix::read_mix();
                                let mut h = Hists::default();
                                let mut n = 0;
                                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    let op = mix::pick(&mut rng, &rmix);
                                    let t = Instant::now();
                                    let _ = mix::run_op(s.as_mut(), st, op, Dur::Relaxed, &mut rng);
                                    h.rec("read", t.elapsed().as_nanos() as u64);
                                    n += 1;
                                }
                                Ok((h, n))
                            })
                        })
                        .collect();
                    let w = mix::drive(a.as_ref(), &st, &wcfg);
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    let mut h = Hists::default();
                    let mut n = 0;
                    for t in readers {
                        let (hh, nn) = t.join().expect("reader")?;
                        h.merge(&hh);
                        n += nn;
                    }
                    Ok((w?, (h, n)))
                })?;
                let secs = w.wall_s;
                let all_w = w.latency.all();
                series.push("writer_p99_us", all_w.value_at_quantile(0.99) as f64 / 1e3);
                series.push("writer_achieved_tps", w.ops as f64 / secs);
                series.push("reader_ops_per_s", rd.1 as f64 / secs);
                if let Some(h) = rd.0.by_class.get("read") {
                    series.push("reader_p99_us", h.value_at_quantile(0.99) as f64 / 1e3);
                }
                series.push("file_growth_bytes", a.disk_size() as f64 - size0 as f64);
                runs.push(json!({"run": run, "writer": w.to_json(), "readers": rd.0.to_json(), "reader_ops": rd.1}));
            }
            points.push(json!({"readers": nr, "summary": series.to_json(), "runs": runs}));
        }
        out.push(json!({"adapter": e.name(), "describe": a.describe(), "points": points}));
        drop(a);
        c.cleanup(&dir);
    }
    c.save("reader-scaling", json!(format!("{r:?}")), json!({"adapters": out}))
}

// ---------------------------------------------------------------- open and recovery

#[derive(Clone, Debug, clap::Args)]
pub struct OpenArgs {
    #[arg(long, value_delimiter = ',', default_value = "1000000,10000000")]
    pub sizes: Vec<u64>,
    /// Statements in the transaction killed mid-flight (crash case).
    #[arg(long, default_value_t = 200_000)]
    pub crash_txn: u64,
}

fn time_open(c: &Ctx, e: Engine, dir: &std::path::Path) -> Result<Value> {
    let rss0 = crate::env::max_rss();
    let t0 = Instant::now();
    let a = c.open(e, dir)?;
    let open_ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut s = a.session()?;
    let found = s.node(&gen::node_iri(0))?.map(|h| s.props(h)).transpose()?;
    let first_ms = t0.elapsed().as_secs_f64() * 1e3;
    Ok(json!({"open_ms": open_ms, "time_to_first_query_ms": first_ms, "first_query_rows": found,
        "max_rss_growth_bytes": crate::env::max_rss().saturating_sub(rss0),
        "pages_read": null, "pages_read_note": "not measured: needs OS I/O tracing (fs_usage/iostat or /proc/self/io read_bytes)"}))
}

pub fn open_recovery(c: &Ctx, o: &OpenArgs) -> Result<()> {
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        for &n in &o.sizes {
            let dir = c.fresh_dir(&format!("{}-open{n}", e.name()))?;
            {
                let a = c.open(e, &dir)?;
                gen::load(a.as_ref(), &Shape::new(n, c.c.seed), 5000)?;
            }
            let mut clean = Vec::new();
            for _ in 0..c.c.runs {
                if c.c.cache == crate::run::Cache::Cold {
                    crate::env::drop_caches();
                }
                clean.push(time_open(c, e, &dir)?);
            }
            // Crash case: a child process runs a large relaxed transaction and is killed.
            let exe = std::env::current_exe()?;
            let mut child = std::process::Command::new(exe)
                .args(["crash-child", "--engine", e.name(), "--dir"])
                .arg(&dir)
                .args(["--statements", &o.crash_txn.to_string()])
                .spawn()?;
            std::thread::sleep(Duration::from_millis(500));
            let _ = child.kill(); // SIGKILL on Unix
            let _ = child.wait();
            // A failed reopen is a result, not a harness error.
            let crashed = time_open(c, e, &dir).unwrap_or_else(|x| json!({"error": format!("{x:#}")}));
            let ms: Vec<f64> = clean.iter().filter_map(|v| v["time_to_first_query_ms"].as_f64()).collect();
            out.push(json!({"adapter": e.name(), "statements": n, "clean": clean,
                "clean_summary": crate::stats::summarize(&ms), "after_kill9": crashed,
                "file_bytes": crate::env::path_size(&dir)}));
            c.cleanup(&dir);
        }
    }
    c.save("open-recovery", json!(format!("{o:?}")), json!({"points": out}))
}

/// Hidden subcommand: writes one big relaxed transaction until killed.
pub fn crash_child(c: &Ctx, e: Engine, dir: &std::path::Path, n: u64) -> Result<()> {
    let a = c.open(e, dir)?;
    let mut s = a.session()?;
    let ops: Vec<_> = (0..n)
        .map(|i| crate::adapter::WOp::Insert {
            s: crate::adapter::Term::Iri(format!("{}crash/{i}", gen::BASE)),
            p: gen::pred(0),
            o: crate::adapter::Term::int(i as i64),
            g: None,
        })
        .collect();
    loop {
        s.write(&ops, Dur::Relaxed)?;
    }
}

// ---------------------------------------------------------------- space under churn

#[derive(Clone, Debug, clap::Args)]
pub struct ChurnArgs {
    #[arg(long, default_value_t = 10_000_000)]
    pub statements: u64,
    /// Overwrite and annotate operations in total.
    #[arg(long, default_value_t = 100_000_000)]
    pub ops: u64,
    /// Measurement points.
    #[arg(long, default_value_t = 10)]
    pub points: u64,
}

pub fn churn(c: &Ctx, ch: &ChurnArgs) -> Result<()> {
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let dir = c.fresh_dir(&format!("{}-churn", e.name()))?;
        let a = c.open(e, &dir)?;
        let shape = Shape::new(ch.statements, c.c.seed);
        let loaded = gen::load(a.as_ref(), &shape, 5000)?;
        let live = loaded.load["statements"].as_f64().unwrap_or(1.0);
        let after_load = a.disk_size();
        let st = State::new(loaded);
        let churn_mix = vec![(mix::Op::WriteTxn, 80), (mix::Op::Annotate, 20)];
        let mut pts = vec![json!({"ops": 0, "disk_bytes": after_load, "bytes_per_live_statement": after_load as f64 / live})];
        let step = (ch.ops / ch.points.max(1)).max(1);
        for i in 1..=ch.points {
            let cfg = DriveCfg {
                ops: step,
                warmup: 0,
                threads: 1,
                rate: None,
                dur: Dur::Relaxed,
                seed: c.c.seed + i,
                mix: churn_mix.clone(),
            };
            let r = mix::drive(a.as_ref(), &st, &cfg)?;
            a.checkpoint()?;
            let sz = a.disk_size();
            pts.push(json!({"ops": i * step, "disk_bytes": sz, "ratio_to_load": sz as f64 / after_load.max(1) as f64,
                "bytes_per_live_statement": sz as f64 / live, "ops_per_s": r.ops as f64 / r.wall_s}));
            eprintln!("[{}] churn {}/{}: {} bytes", e.name(), i, ch.points, sz);
        }
        out.push(json!({"adapter": e.name(), "after_load_bytes": after_load, "points": pts,
            "free_page_ratio": null, "compaction": null,
            "gap": "graphlava exposes no compaction or free-page counter in its public API yet; add them when Admin gains compact()"}));
        drop(a);
        c.cleanup(&dir);
    }
    c.save("space-churn", json!(format!("{ch:?}")), json!({"adapters": out}))
}
