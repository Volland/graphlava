//! Shared options, the run context, and the generic "N runs per
//! configuration" loop used by every subcommand.

use crate::adapter::{self, Adapter, Dur, Engine, OpenCfg};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cache {
    Warm,
    Cold,
}

/// Options every benchmark accepts.
#[derive(Clone, Debug, clap::Args)]
pub struct Common {
    /// Engines to run, comma separated.
    #[arg(long, value_delimiter = ',', default_value = "graphlava")]
    pub adapters: Vec<Engine>,
    /// Runs per configuration (the spec asks for at least 5).
    #[arg(long, default_value_t = 5)]
    pub runs: usize,
    /// Durability of timed writes.
    #[arg(long, value_enum, default_value_t = Dur::Ordered)]
    pub durability: Dur,
    /// Cache state: `cold` reopens the engine and drops the OS page cache
    /// before every run (needs non-interactive sudo; else recorded as warm).
    #[arg(long, value_enum, default_value_t = Cache::Warm)]
    pub cache: Cache,
    /// Generator / driver seed.
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// Directory for database files (default bench/data/work).
    #[arg(long)]
    pub work_dir: Option<PathBuf>,
    /// Directory for result files (default bench/results).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Do not write a result file (smoke tests).
    #[arg(long)]
    pub no_save: bool,
    /// Keep the database files after the run.
    #[arg(long)]
    pub keep: bool,
    /// graphlava group-commit window in ms.
    #[arg(long, default_value_t = 2)]
    pub group_window_ms: u64,
    /// Engine page cache in bytes (graphlava cache_bytes; SQLite uses 64 MiB).
    #[arg(long, default_value_t = 64 << 20)]
    pub cache_bytes: usize,
}

pub fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

pub struct Ctx {
    pub c: Common,
    pub work: PathBuf,
    pub env: Value,
}

impl Ctx {
    pub fn new(c: Common) -> Result<Ctx> {
        let work = c
            .work_dir
            .clone()
            .unwrap_or_else(|| bench_dir().join("data").join("work"));
        std::fs::create_dir_all(&work)?;
        let env = crate::env::capture(&work);
        Ok(Ctx { c, work, env })
    }

    /// A fresh directory for one engine and configuration.
    pub fn fresh_dir(&self, name: &str) -> Result<PathBuf> {
        let d = self.work.join(name);
        if d.exists() {
            std::fs::remove_dir_all(&d)?;
        }
        std::fs::create_dir_all(&d)?;
        Ok(d)
    }

    pub fn cfg(&self, dir: &Path) -> OpenCfg {
        OpenCfg {
            dir: dir.to_path_buf(),
            dur: self.c.durability,
            group_window_ms: self.c.group_window_ms,
            cache_bytes: self.c.cache_bytes,
        }
    }

    pub fn open(&self, e: Engine, dir: &Path) -> Result<Box<dyn Adapter>> {
        adapter::open(e, &self.cfg(dir))
    }

    /// Before a timed run: in cold mode reopen the engine and drop caches.
    /// Returns the cache state actually achieved.
    pub fn prepare_run(&self, e: Engine, dir: &Path, a: &mut Option<Box<dyn Adapter>>) -> Result<&'static str> {
        if self.c.cache == Cache::Warm {
            return Ok("warm");
        }
        *a = None;
        let ok = crate::env::drop_caches();
        *a = Some(self.open(e, dir)?);
        Ok(if ok { "cold" } else { "warm (cache drop not permitted)" })
    }

    pub fn cleanup(&self, dir: &Path) {
        if !self.c.keep {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Writes the result (unless `--no-save`) and prints a short summary.
    pub fn save(&self, benchmark: &str, config: Value, body: Value) -> Result<()> {
        let config = json!({"common": {
            "adapters": self.c.adapters.iter().map(|e| e.name()).collect::<Vec<_>>(),
            "runs": self.c.runs, "durability": self.c.durability, "cache": self.c.cache,
            "seed": self.c.seed, "group_window_ms": self.c.group_window_ms, "cache_bytes": self.c.cache_bytes,
            "generator": crate::gen::GEN_VERSION, "mix": crate::mix::MIX_VERSION,
        }, "benchmark": config});
        if self.c.no_save {
            eprintln!("(--no-save) {benchmark}: result not written");
            println!("{}", serde_json::to_string_pretty(&summary_only(&body))?);
            return Ok(());
        }
        let out = self
            .c
            .out
            .clone()
            .unwrap_or_else(|| bench_dir().join("results"));
        let p = crate::results::append(&out, benchmark, &self.env, config, body)?;
        eprintln!("{benchmark}: appended to {}", p.display());
        Ok(())
    }
}

/// The body without raw histograms, for printing.
pub fn summary_only(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| k.as_str() != "hdr_v2_deflate_b64" && k.as_str() != "runs")
                .map(|(k, v)| (k.clone(), summary_only(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(summary_only).collect()),
        x => x.clone(),
    }
}

/// Whether an adapter honours the requested durability exactly.
pub fn durability_matches(e: Engine, d: Dur) -> bool {
    match e {
        Engine::Graphlava => true,
        Engine::Sqlite => d != Dur::Ordered,
        Engine::Oxigraph => d == Dur::Relaxed,
        Engine::Tiramemsu => d != Dur::Ordered,
    }
}
