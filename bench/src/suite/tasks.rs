//! The three graphlava-specific benchmarks of task 2.5:
//! [[17-benchmarks#Inline literal range scan]],
//! [[17-benchmarks#Durability commit latency]],
//! [[17-benchmarks#Dictionary sweep cost]].

use crate::adapter::{graphlava as glv, Dur, Engine, Term};
use crate::run::Ctx;
use crate::stats::{self, Hists, RunSeries};
use anyhow::{anyhow, Result};
use graphlava::{CoreOp, CrashMode, Database, Durability, FaultPlan, OpenOptions, WriteRequest};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const B: &str = "http://bench.graphlava.org/";

fn iri(s: String) -> graphlava::TermRef {
    graphlava::Term::iri(s).into()
}

/// `xsd:dateTime` lexical form of 2020-01-01T00:00:00Z + `secs`.
pub fn date_time(secs: i64) -> String {
    let t = 1_577_836_800 + secs;
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// ---------------------------------------------------------------- range scan

#[derive(Clone, Debug, clap::Args)]
pub struct RangeArgs {
    /// Nodes, each with an integer, a dateTime, an inline double and a dictionary double.
    #[arg(long, default_value_t = 10_000_000)]
    pub nodes: u64,
    /// Selectivities of `?v > c`.
    #[arg(long, value_delimiter = ',', default_value = "0.0001,0.01,0.1")]
    pub selectivities: Vec<f64>,
    /// Queries per (type, selectivity, index) point and run.
    #[arg(long, default_value_t = 20)]
    pub queries: u64,
}

/// One property type of the range workload: predicate local name and the
/// literal for rank `v` (rank order equals value order).
type Rank = fn(u64) -> Term;

fn range_types() -> Vec<(&'static str, Rank)> {
    vec![
        ("int", |v| Term::int(v as i64)),
        ("dateTime", |v| Term::typed(date_time(v as i64), "dateTime")),
        // |exponent| < 255: inline doubles.
        ("double_inline", |v| Term::typed(format!("{:E}", v as f64 * 0.5 + 0.25), "double")),
        // exponent ~ -1000 is outside the inline range: dictionary doubles.
        ("double_dict", |v| Term::typed(format!("{:E}", (v as f64 + 1.0) * 1e-300), "double")),
    ]
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

pub fn range_scan(c: &Ctx, r: &RangeArgs) -> Result<()> {
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let dir = c.fresh_dir(&format!("{}-range", e.name()))?;
        let a = c.open(e, &dir)?;
        let types = range_types();
        // Load: value rank v = a fixed permutation of node index.
        let mut mult = 7919u64;
        while gcd(mult, r.nodes) != 1 {
            mult += 2;
        }
        let t0 = Instant::now();
        {
            let mut s = a.session()?;
            let mut ops = Vec::with_capacity(20_000);
            for i in 0..r.nodes {
                // Bijective rank: i·mult mod nodes with gcd(mult, nodes) = 1.
                let v = (i * mult + 13) % r.nodes;
                for (name, f) in &types {
                    ops.push(crate::adapter::WOp::Insert {
                        s: Term::Iri(format!("{B}rn/{i}")),
                        p: Term::Iri(format!("{B}range/{name}")),
                        o: f(v),
                        g: None,
                    });
                }
                if ops.len() >= 20_000 {
                    s.write(&ops, Dur::Relaxed)?;
                    ops.clear();
                }
            }
            if !ops.is_empty() {
                s.write(&ops, Dur::Relaxed)?;
            }
        }
        a.checkpoint()?;
        let load_s = t0.elapsed().as_secs_f64();
        let mut points = Vec::new();
        for indexed in [false, true] {
            if indexed {
                match a.as_graphlava() {
                    Some(g) => {
                        let t = Instant::now();
                        for (name, _) in &types {
                            g.db().admin().build_value_index(&graphlava::Term::iri(format!("{B}range/{name}")))?;
                        }
                        eprintln!("[{}] value indexes built in {:.1}s", e.name(), t.elapsed().as_secs_f64());
                    }
                    None => continue, // SQLite/Oxigraph: their indexes are always on
                }
            }
            for (name, f) in &types {
                let p = Term::Iri(format!("{B}range/{name}"));
                for &sel in &r.selectivities {
                    let c_rank = ((r.nodes as f64) * (1.0 - sel)).floor() as u64;
                    let lo = f(c_rank.min(r.nodes - 1));
                    let mut series = RunSeries::default();
                    let mut runs = Vec::new();
                    for path in ["native", "sparql"] {
                        for run in 0..c.c.runs {
                            let mut s = a.session()?;
                            let mut h = Hists::default();
                            let mut rows = 0usize;
                            let mut err = None;
                            for _ in 0..r.queries {
                                let t = Instant::now();
                                let res = if path == "native" {
                                    s.range_gt(&p, &lo)
                                } else {
                                    s.sparql(&format!("SELECT ?s WHERE {{ ?s {} ?v FILTER(?v > {}) }}", p.nt(), lo.nt()))
                                };
                                h.rec(path, t.elapsed().as_nanos() as u64);
                                match res {
                                    Ok(n) => rows = n,
                                    Err(x) => {
                                        err = Some(format!("{x:#}"));
                                        break;
                                    }
                                }
                            }
                            if err.is_none() {
                                series.push_hists(&h);
                            }
                            runs.push(json!({"run": run, "path": path, "rows_returned": rows, "error": err, "latency": h.to_json()}));
                        }
                    }
                    points.push(json!({
                        "type": name, "selectivity": sel, "value_index": indexed,
                        "expected_rows": r.nodes - c_rank - 1,
                        "rows_scanned_per_row_returned": null,
                        "dictionary_decode_share": if *name == "double_dict" { json!("all values (dictionary doubles)") } else { json!(0.0) },
                        "summary": series.to_json(), "runs": runs,
                    }));
                }
            }
        }
        out.push(json!({"adapter": e.name(), "describe": a.describe(), "load_s": load_s,
            "disk_bytes": a.disk_size(), "points": points,
            "gaps": ["rows scanned per row returned is not exposed by graphlava's public API (needs an executor counter)",
                     "Reader::find_range serves inline values only; dictionary doubles are measured through SPARQL FILTER"]}));
        drop(a);
        c.cleanup(&dir);
    }
    c.save("range-scan", json!(format!("{r:?}")), json!({"adapters": out}))
}

// ---------------------------------------------------------------- commit latency

#[derive(Clone, Debug, clap::Args)]
pub struct CommitArgs {
    #[arg(long, value_delimiter = ',', default_value = "1,8,64")]
    pub callers: Vec<usize>,
    /// Group-commit windows in ms.
    #[arg(long, value_delimiter = ',', default_value = "1,2,5")]
    pub windows_ms: Vec<u64>,
    #[arg(long, value_delimiter = ',', default_value = "sync,ordered,relaxed")]
    pub levels: Vec<Dur>,
    /// Seconds per configuration and run.
    #[arg(long, default_value_t = 5.0)]
    pub secs: f64,
    /// Batch sizes of the bytes-per-request measurement.
    #[arg(long, value_delimiter = ',', default_value = "1,8,64")]
    pub batch_sizes: Vec<usize>,
    /// Rounds of the bytes-per-request measurement.
    #[arg(long, default_value_t = 64)]
    pub rounds: usize,
}

fn preload(path: &std::path::Path) -> Result<()> {
    let db = Database::open(path, OpenOptions::default())?;
    let ops = (0..20_000)
        .map(|i| CoreOp::Insert {
            s: iri(format!("{B}c/s{i}")),
            p: iri(format!("{B}c/p")),
            o: iri(format!("{B}c/o{}", i % 97)),
            graph: None,
        })
        .collect();
    db.submit(WriteRequest { ops }, Durability::Sync)?;
    db.close()?;
    Ok(())
}

fn request(rng: &mut SmallRng, caller: usize, k: u64) -> WriteRequest {
    let n = rng.gen_range(1..=20);
    WriteRequest {
        ops: (0..n)
            .map(|j| CoreOp::Insert {
                s: iri(format!("{B}c/w{caller}-{k}-{j}")),
                p: iri(format!("{B}c/q{}", j % 8)),
                o: graphlava::Term::integer(rng.gen_range(0..1_000_000)).into(),
                graph: None,
            })
            .collect(),
    }
}

/// Bytes written to the file per request at a group-commit batch size,
/// counted by the I/O-counting backend (as `examples/batch_bytes.rs`).
fn batch_bytes(work: &std::path::Path, batch: usize, rounds: usize) -> Result<Value> {
    let dir = tempfile::tempdir_in(work)?;
    let path = dir.path().join("b.glv");
    let count = dir.path().join("count");
    preload(&path)?;
    let opts = OpenOptions {
        group_window: Duration::from_millis(if batch == 1 { 0 } else { 20 }),
        fault: Some(FaultPlan { abort_at: 0, mode: CrashMode::ProcessKill, count_file: Some(count.clone()) }),
        ..OpenOptions::default()
    };
    let db = Arc::new(Database::open(&path, opts)?);
    let mut batches = std::collections::HashSet::new();
    for r in 0..rounds {
        let gate = Arc::new(Barrier::new(batch));
        let hs: Vec<_> = (0..batch)
            .map(|t| {
                let (db, gate) = (Arc::clone(&db), Arc::clone(&gate));
                std::thread::spawn(move || -> Result<u64> {
                    gate.wait();
                    let op = CoreOp::Insert {
                        s: iri(format!("{B}b/r{r}t{t}")),
                        p: iri(format!("{B}b/q")),
                        o: iri(format!("{B}b/o{t}")),
                        graph: None,
                    };
                    Ok(db.submit(WriteRequest { ops: vec![op] }, Durability::Ordered)?.batch)
                })
            })
            .collect();
        for h in hs {
            batches.insert(h.join().map_err(|_| anyhow!("thread"))??);
        }
    }
    Arc::try_unwrap(db).map_err(|_| anyhow!("db still shared"))?.close()?;
    let txt = std::fs::read_to_string(&count)?;
    let bytes: f64 = txt.lines().nth(1).ok_or_else(|| anyhow!("count file"))?.trim().parse()?;
    let reqs = (rounds * batch) as f64;
    Ok(json!({"batch_target": batch, "requests_per_batch_achieved": reqs / batches.len() as f64,
        "bytes_written_per_request": bytes / reqs, "engine_transactions": batches.len()}))
}

pub fn commit_latency(c: &Ctx, a: &CommitArgs) -> Result<()> {
    if c.c.adapters.iter().any(|e| *e != Engine::Graphlava) {
        eprintln!("commit-latency measures graphlava's group commit only; other adapters ignored");
    }
    let mut points = Vec::new();
    for &w in &a.windows_ms {
        for &lvl in &a.levels {
            for &callers in &a.callers {
                let mut series = RunSeries::default();
                let mut runs = Vec::new();
                for run in 0..c.c.runs {
                    let dir = c.fresh_dir("graphlava-commit")?;
                    let path = dir.join("c.glv");
                    preload(&path)?;
                    let db = Database::open(&path, OpenOptions { group_window: Duration::from_millis(w), ..OpenOptions::default() })?;
                    let stop = AtomicBool::new(false);
                    let reqs = AtomicU64::new(0);
                    let commits0 = db.durable_commits();
                    let d0 = crate::env::disk_bytes_written();
                    let t0 = Instant::now();
                    let hs: Vec<Hists> = std::thread::scope(|sc| {
                        let ths: Vec<_> = (0..callers)
                            .map(|ci| {
                                let (db, stop, reqs) = (&db, &stop, &reqs);
                                sc.spawn(move || {
                                    let mut rng = SmallRng::seed_from_u64(c.c.seed + ci as u64 + 100 * run as u64);
                                    let mut h = Hists::default();
                                    let mut k = 0;
                                    while !stop.load(Ordering::Relaxed) {
                                        let req = request(&mut rng, ci, k);
                                        k += 1;
                                        let t = Instant::now();
                                        if db.submit(req, glv::dur(lvl)).is_ok() {
                                            h.rec("ack", t.elapsed().as_nanos() as u64);
                                            reqs.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                    h
                                })
                            })
                            .collect();
                        std::thread::sleep(Duration::from_secs_f64(a.secs));
                        stop.store(true, Ordering::Relaxed);
                        ths.into_iter().map(|t| t.join().expect("caller")).collect()
                    });
                    let secs = t0.elapsed().as_secs_f64();
                    let mut h = Hists::default();
                    for x in &hs {
                        h.merge(x);
                    }
                    let n = reqs.load(Ordering::Relaxed) as f64;
                    let durable = (db.durable_commits() - commits0) as f64;
                    let written = crate::env::disk_bytes_written().zip(d0).map(|(b, a)| b.saturating_sub(a));
                    db.close()?;
                    series.push_hists(&h);
                    series.push("requests_per_s", n / secs);
                    series.push("durable_commits_per_s", durable / secs);
                    runs.push(json!({"run": run, "requests": n, "secs": secs, "durable_commits": durable,
                        "os_disk_bytes_written": written, "latency": h.to_json()}));
                    c.cleanup(&dir);
                }
                eprintln!("[commit] window {w}ms {} callers {callers}: done", lvl.name());
                points.push(json!({"window_ms": w, "durability": lvl, "callers": callers,
                    "summary": series.to_json(), "runs": runs}));
            }
        }
    }
    let mut bb = Vec::new();
    for &b in &a.batch_sizes {
        let mut per = Vec::new();
        for _ in 0..c.c.runs {
            per.push(batch_bytes(&c.work, b, a.rounds)?);
        }
        let v: Vec<f64> = per.iter().filter_map(|x| x["bytes_written_per_request"].as_f64()).collect();
        bb.push(json!({"batch": b, "bytes_per_request": stats::summarize(&v), "runs": per}));
    }
    c.save("commit-latency", json!(format!("{a:?}")), json!({
        "points": points,
        "bytes_per_request": bb,
        "fsync_note": "fsync calls/s is approximated by durable batch commits/s (Database::durable_commits); a redb durable commit issues its own fsyncs, count them with dtruss/strace on the fixed runner",
    }))
}

// ---------------------------------------------------------------- sweep cost

#[derive(Clone, Debug, clap::Args)]
pub struct SweepArgs {
    /// Statements (node-valued edges plus term-valued statements).
    #[arg(long, default_value_t = 10_000_000)]
    pub statements: u64,
    /// Dictionary terms created (one long string literal per term statement).
    #[arg(long, default_value_t = 1_000_000)]
    pub terms: u64,
    /// Fraction of terms left unreferenced by the churn phase.
    #[arg(long, default_value_t = 0.1)]
    pub unreferenced: f64,
    /// Writer arrival rate during the idle and sweep phases (requests/s).
    #[arg(long, default_value_t = 1000.0)]
    pub writer_rate: f64,
    /// Seconds of the idle-writer phase.
    #[arg(long, default_value_t = 10.0)]
    pub idle_secs: f64,
}

fn term_lit(k: u64) -> graphlava::Term {
    // Long enough to never be an inline string.
    graphlava::Term::string(format!("dictionary-term-{k:012}-padding-to-force-a-dictionary-entry"))
}

/// Open-loop writer until `stop`, recording ack latency from intended start.
fn writer(db: &Database, rate: f64, dur: Dur, stop: &AtomicBool, seed: u64) -> Hists {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut h = Hists::default();
    let t0 = Instant::now();
    let mut i = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let t = t0 + Duration::from_secs_f64(i as f64 / rate);
        while Instant::now() < t {
            std::thread::sleep(Duration::from_micros(50));
        }
        let _ = db.submit(request(&mut rng, 0, i), glv::dur(dur));
        h.rec("write", (Instant::now() - t).as_nanos() as u64);
        i += 1;
    }
    h
}

pub fn sweep_cost(c: &Ctx, a: &SweepArgs) -> Result<()> {
    let mut series = RunSeries::default();
    let mut runs = Vec::new();
    for run in 0..c.c.runs {
        let dir = c.fresh_dir("graphlava-sweep")?;
        let path = dir.join("s.glv");
        let db = Database::open(&path, OpenOptions::default())?;
        let t0 = Instant::now();
        let edges = a.statements.saturating_sub(a.terms);
        let mut ops = Vec::with_capacity(20_000);
        let flush = |ops: &mut Vec<CoreOp>| -> Result<()> {
            db.submit(WriteRequest { ops: std::mem::take(ops) }, Durability::Relaxed)?;
            Ok(())
        };
        for k in 0..a.terms {
            ops.push(CoreOp::Insert { s: iri(format!("{B}sw/n{}", k % (a.terms / 4).max(1))), p: iri(format!("{B}sw/label")), o: term_lit(k).into(), graph: None });
            if ops.len() >= 20_000 { flush(&mut ops)?; }
        }
        for k in 0..edges {
            ops.push(CoreOp::Insert { s: iri(format!("{B}sw/n{}", k % 100_000)), p: iri(format!("{B}sw/e{}", k % 16)), o: iri(format!("{B}sw/n{}", (k * 7919) % 100_000)), graph: None });
            if ops.len() >= 20_000 { flush(&mut ops)?; }
        }
        if !ops.is_empty() { flush(&mut ops)?; }
        // Churn: remove the statements of every 1/fraction-th term.
        let step = (1.0 / a.unreferenced.max(1e-9)).round().max(1.0) as u64;
        let mut removed = 0u64;
        for k in (0..a.terms).step_by(step as usize) {
            ops.push(CoreOp::Remove { s: iri(format!("{B}sw/n{}", k % (a.terms / 4).max(1))), p: iri(format!("{B}sw/label")), o: term_lit(k).into(), mode: graphlava::DeleteMode::Cascade, graph: None });
            removed += 1;
            if ops.len() >= 20_000 { flush(&mut ops)?; }
        }
        if !ops.is_empty() { flush(&mut ops)?; }
        db.submit(WriteRequest::default(), Durability::Sync)?;
        let load_s = t0.elapsed().as_secs_f64();
        let size0 = db.file_size()?;
        // Idle writer.
        let stop = AtomicBool::new(false);
        let idle = std::thread::scope(|sc| {
            let h = sc.spawn(|| writer(&db, a.writer_rate, c.c.durability, &stop, c.c.seed + run as u64));
            std::thread::sleep(Duration::from_secs_f64(a.idle_secs));
            stop.store(true, Ordering::Relaxed);
            h.join().expect("writer")
        });
        // Writer while sweeping.
        let stop = AtomicBool::new(false);
        let (during, rep, sweep_s) = std::thread::scope(|sc| {
            let h = sc.spawn(|| writer(&db, a.writer_rate, c.c.durability, &stop, c.c.seed + 99 + run as u64));
            let t = Instant::now();
            let rep = db.admin().sweep(Duration::ZERO);
            let s = t.elapsed().as_secs_f64();
            stop.store(true, Ordering::Relaxed);
            (h.join().expect("writer"), rep, s)
        });
        let rep = rep?;
        let size1 = db.file_size()?;
        db.close()?;
        let p99 = |h: &Hists| h.by_class.get("write").map_or(f64::NAN, |x| x.value_at_quantile(0.99) as f64 / 1e3);
        let (pi, pd) = (p99(&idle), p99(&during));
        series.push("sweep_s", sweep_s);
        series.push("sweep_s_per_1e6_terms", sweep_s / (rep.examined.max(1) as f64 / 1e6));
        series.push("writer_p99_idle_us", pi);
        series.push("writer_p99_sweep_us", pd);
        series.push("writer_p99_ratio", pd / pi);
        series.push("terms_deleted", rep.deleted as f64);
        runs.push(json!({"run": run, "load_and_churn_s": load_s, "statements_removed": removed,
            "sweep_report": {"marked": rep.marked, "examined": rep.examined, "deleted": rep.deleted,
                "kept_young": rep.kept_young, "kept_rereferenced": rep.kept_rereferenced},
            "file_bytes_before": size0, "file_bytes_after": size1,
            "writer_idle": idle.to_json(), "writer_during_sweep": during.to_json()}));
        eprintln!("[sweep] run {run}: {sweep_s:.2}s, deleted {}, p99 idle {pi:.0}us sweep {pd:.0}us", rep.deleted);
        c.cleanup(&dir);
    }
    c.save("sweep-cost", json!(format!("{a:?}")), json!({"summary": series.to_json(), "runs": runs,
        "bytes_reclaimed_note": "redb does not shrink the file; reclaimed space shows as deleted dictionary entries and stable file size under later writes"}))
}
