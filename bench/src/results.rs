//! Result files: `bench/results/<date>-<commit>-<host>.json`, one per day,
//! commit and host; every benchmark invocation appends one entry to
//! `benchmarks` ([[17-benchmarks#Harness and method]]). Schema in bench/README.md.

use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "meta-bench/v1";

/// Today's UTC date as YYYY-MM-DD and the RFC 3339 timestamp.
pub fn now() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let date = format!("{y:04}-{m:02}-{d:02}");
    let ts = format!(
        "{date}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    (date, ts)
}

/// Appends one benchmark entry and returns the file written.
pub fn append(out_dir: &Path, benchmark: &str, env: &Value, config: Value, body: Value) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let (date, ts) = now();
    let commit = env["graphlava_commit"].as_str().unwrap_or("unknown");
    let short = &commit[..commit.len().min(12)];
    let host = env["host"].as_str().unwrap_or("host");
    let path = out_dir.join(format!("{date}-{short}-{host}.json"));
    let mut doc: Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s)?,
        Err(_) => json!({
            "schema": SCHEMA,
            "date": date,
            "graphlava_commit": commit,
            "host": host,
            "benchmarks": [],
        }),
    };
    let entry = json!({
        "benchmark": benchmark,
        "created": ts,
        "env": env,
        "config": config,
        "result": body,
    });
    doc["benchmarks"]
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("{}: no benchmarks array", path.display()))?
        .push(entry);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}
