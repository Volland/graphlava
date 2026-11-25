//! `meta-bench`: the macro benchmark harness of graphlava
//! ([[17-benchmarks#Harness and method]], [[adr:0020]]). See bench/README.md.

mod adapter;
mod env;
mod gen;
mod mix;
mod results;
mod run;
mod stats;
mod suite;
mod workload;

use anyhow::Result;
use clap::{Parser, Subcommand};
use run::{Common, Ctx};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "meta-bench", version, about = "graphlava macro benchmarks")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print the environment block a result would record.
    Env {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Write the glv-agent-v1 dataset as N-Quads.
    Gen {
        #[arg(long, default_value_t = 1_000_000)]
        statements: u64,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// The agent-memory mix (open loop with --rate, closed loop with --rate 0).
    Mix {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        m: suite::mixbench::MixArgs,
    },
    /// Inline literal range scan (task 2.5).
    RangeScan {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::tasks::RangeArgs,
    },
    /// Durability commit latency (task 2.5; graphlava only).
    CommitLatency {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::tasks::CommitArgs,
    },
    /// Dictionary sweep cost (task 2.5; graphlava only).
    SweepCost {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::tasks::SweepArgs,
    },
    /// Scaling sweep (nightly).
    Scaling {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::mixbench::ScalingArgs,
    },
    /// Reader scaling (nightly).
    Readers {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::mixbench::ReadersArgs,
    },
    /// Open and recovery time (nightly).
    OpenRecovery {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::mixbench::OpenArgs,
    },
    /// Space under churn (nightly).
    Churn {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: suite::mixbench::ChurnArgs,
    },
    /// LUBM 14 queries, with and without RDFS rules.
    Lubm {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: workload::lubm::LubmArgs,
    },
    /// WatDiv basic templates.
    Watdiv {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: workload::watdiv::WatdivArgs,
    },
    /// BSBM explore and explore-and-update mixes.
    Bsbm {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: workload::bsbm::BsbmArgs,
    },
    /// SNB-style short reads and inserts.
    Snb {
        #[command(flatten)]
        c: Common,
        #[command(flatten)]
        a: workload::snb::SnbArgs,
    },
    /// Internal: big relaxed transaction until killed (open-recovery).
    #[command(hide = true)]
    CrashChild {
        #[arg(long, value_enum)]
        engine: adapter::Engine,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        statements: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Env { dir } => {
            let d = dir.unwrap_or_else(|| run::bench_dir().join("data"));
            std::fs::create_dir_all(&d)?;
            println!("{}", serde_json::to_string_pretty(&env::capture(&d))?);
            Ok(())
        }
        Cmd::Gen { statements, seed, out } => {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&out)?);
            let n = gen::write_nquads(&gen::Shape::new(statements, seed), &mut w)?;
            eprintln!("{} statements ({}) -> {}", n, gen::GEN_VERSION, out.display());
            Ok(())
        }
        Cmd::Mix { c, m } => suite::mixbench::mix(&Ctx::new(c)?, &m),
        Cmd::RangeScan { c, a } => suite::tasks::range_scan(&Ctx::new(c)?, &a),
        Cmd::CommitLatency { c, a } => suite::tasks::commit_latency(&Ctx::new(c)?, &a),
        Cmd::SweepCost { c, a } => suite::tasks::sweep_cost(&Ctx::new(c)?, &a),
        Cmd::Scaling { c, a } => suite::mixbench::scaling(&Ctx::new(c)?, &a),
        Cmd::Readers { c, a } => suite::mixbench::readers(&Ctx::new(c)?, &a),
        Cmd::OpenRecovery { c, a } => suite::mixbench::open_recovery(&Ctx::new(c)?, &a),
        Cmd::Churn { c, a } => suite::mixbench::churn(&Ctx::new(c)?, &a),
        Cmd::Lubm { c, a } => workload::lubm::run(&Ctx::new(c)?, &a),
        Cmd::Watdiv { c, a } => workload::watdiv::run(&Ctx::new(c)?, &a),
        Cmd::Bsbm { c, a } => workload::bsbm::run(&Ctx::new(c)?, &a),
        Cmd::Snb { c, a } => workload::snb::run(&Ctx::new(c)?, &a),
        Cmd::CrashChild { engine, dir, statements } => {
            let c = Common {
                adapters: vec![engine],
                runs: 1,
                durability: adapter::Dur::Relaxed,
                cache: run::Cache::Warm,
                seed: 0,
                work_dir: Some(dir.clone()),
                out: None,
                no_save: true,
                keep: true,
                group_window_ms: 2,
                cache_bytes: 64 << 20,
            };
            let ctx = Ctx { c, work: dir.clone(), env: serde_json::Value::Null };
            suite::mixbench::crash_child(&ctx, engine, &dir, statements)
        }
    }
}
