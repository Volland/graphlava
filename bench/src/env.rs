//! The run environment every result records ([[17-benchmarks#Harness and method]]):
//! hardware, OS and kernel, file system and mount options, storage device,
//! compiler and flags, graphlava commit, cache state. Also the OS counters
//! the workloads read (bytes written, resident memory) and page-cache drops.

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

fn sh(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn sysctl(k: &str) -> Option<String> {
    sh("sysctl", &["-n", k])
}

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Repository root: `bench/..` as built, overridable with `MB_REPO`.
pub fn repo_root() -> std::path::PathBuf {
    std::env::var_os("MB_REPO")
        .map(Into::into)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
}

pub fn git_commit() -> (String, bool) {
    let root = repo_root();
    let r = root.to_string_lossy();
    let head = sh("git", &["-C", &r, "rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = sh("git", &["-C", &r, "status", "--porcelain", "--", "crates"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    (head, dirty)
}

pub fn hostname() -> String {
    sh("hostname", &["-s"])
        .or_else(|| sh("hostname", &[]))
        .unwrap_or_else(|| "host".into())
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_")
}

fn cpu() -> Value {
    let logical = std::thread::available_parallelism().map_or(0, |n| n.get());
    if cfg!(target_os = "macos") {
        json!({
            "model": sysctl("machdep.cpu.brand_string"),
            "physical_cores": sysctl("hw.physicalcpu").and_then(|s| s.parse::<u64>().ok()),
            "logical_cores": logical,
            "perf_levels": sysctl("hw.nperflevels"),
            "p_cores": sysctl("hw.perflevel0.physicalcpu"),
            "e_cores": sysctl("hw.perflevel1.physicalcpu"),
            "ram_bytes": sysctl("hw.memsize").and_then(|s| s.parse::<u64>().ok()),
        })
    } else {
        let info = read("/proc/cpuinfo").unwrap_or_default();
        let model = info
            .lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string());
        let mem = read("/proc/meminfo").and_then(|m| {
            m.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        });
        json!({
            "model": model,
            "physical_cores": sh("sh", &["-c", "lscpu -p=core | grep -v '^#' | sort -u | wc -l"]).and_then(|s| s.parse::<u64>().ok()),
            "logical_cores": logical,
            "ram_bytes": mem,
            "governor": read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        })
    }
}

fn os() -> Value {
    json!({
        "family": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "uname": sh("uname", &["-a"]),
        "kernel": sh("uname", &["-r"]),
        "product": if cfg!(target_os = "macos") {
            sh("sw_vers", &["-productVersion"])
        } else {
            read("/etc/os-release").and_then(|s| s.lines().find(|l| l.starts_with("PRETTY_NAME")).map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string()))
        },
    })
}

/// File system, mount options and device of the directory holding the data.
pub fn storage(dir: &Path) -> Value {
    let d = dir.to_string_lossy().to_string();
    let df = sh("df", &["-P", &d]).unwrap_or_default();
    let line = df.lines().nth(1).unwrap_or("");
    let cols: Vec<&str> = line.split_whitespace().collect();
    let dev = cols.first().copied().unwrap_or("").to_string();
    let mnt = cols.last().copied().unwrap_or("").to_string();
    let mount_line = sh("mount", &[])
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(&format!("{dev} ")))
        .map(str::to_string);
    let (fs, model) = if cfg!(target_os = "macos") {
        let info = sh("diskutil", &["info", &dev]).unwrap_or_default();
        let get = |k: &str| {
            info.lines()
                .find(|l| l.trim_start().starts_with(k))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        };
        (
            get("Type (Bundle)").or(get("File System Personality")),
            json!({
                "media": get("Device / Media Name"),
                "protocol": get("Protocol"),
                "solid_state": get("Solid State"),
                "apfs_physical_store": get("APFS Physical Store"),
            }),
        )
    } else {
        let fs = sh("stat", &["-f", "-c", "%T", &d]);
        let src = sh("findmnt", &["-no", "SOURCE", "--target", &d]).unwrap_or(dev.clone());
        let base = Path::new(&src)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let parent = sh("lsblk", &["-no", "PKNAME", &src]).unwrap_or_default();
        let blk = if parent.is_empty() { base.clone() } else { parent };
        (
            fs,
            json!({
                "model": read(&format!("/sys/block/{blk}/device/model")),
                "rotational": read(&format!("/sys/block/{blk}/queue/rotational")),
                "scheduler": read(&format!("/sys/block/{blk}/queue/scheduler")),
            }),
        )
    };
    json!({"dir": d, "device": dev, "mount_point": mnt, "mount": mount_line, "fs": fs, "device_info": model})
}

pub fn build() -> Value {
    json!({
        "rustc": env!("MB_RUSTC_RELEASE"),
        "rustc_commit": env!("MB_RUSTC_COMMIT"),
        "llvm": env!("MB_RUSTC_LLVM"),
        "host": env!("MB_RUSTC_HOST"),
        "target": env!("MB_TARGET"),
        "profile": env!("MB_PROFILE"),
        "opt_level": env!("MB_OPT_LEVEL"),
        "debug": env!("MB_DEBUG"),
        "rustflags": env!("MB_RUSTFLAGS"),
        "lto": "thin (bench/Cargo.toml [profile.release])",
        "codegen_units": 1,
        "features": env!("MB_FEATURES"),
        "graphlava_version": graphlava::VERSION,
        "oxigraph_version": "0.4.11",
        "rusqlite_version": "0.40",
        "sqlite_version": rusqlite::version(),
    })
}

/// The full environment block.
pub fn capture(data_dir: &Path) -> Value {
    let (commit, dirty) = git_commit();
    json!({
        "host": hostname(),
        "cpu": cpu(),
        "os": os(),
        "storage": storage(data_dir),
        "build": build(),
        "graphlava_commit": commit,
        "graphlava_dirty": dirty,
        "harness_version": env!("CARGO_PKG_VERSION"),
        "load_avg": sh("sh", &["-c", "uptime"]),
    })
}

/// Drops the OS page cache when permitted (`purge` on macOS, `drop_caches`
/// on Linux, both through non-interactive sudo). Returns whether it worked;
/// a failed drop makes the run record `warm`.
pub fn drop_caches() -> bool {
    if cfg!(target_os = "macos") {
        sh("sudo", &["-n", "purge"]).is_some()
    } else {
        let _ = sh("sync", &[]);
        sh("sudo", &["-n", "sh", "-c", "echo 3 > /proc/sys/vm/drop_caches"]).is_some()
    }
}

/// Bytes this process wrote to storage so far (macOS `proc_pid_rusage`,
/// Linux `/proc/self/io` `write_bytes`); `None` when unavailable.
pub fn disk_bytes_written() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: rusage_info_v2 is plain data; the call writes into it.
        unsafe {
            let mut ri: libc::rusage_info_v2 = std::mem::zeroed();
            let rc = libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V2,
                &mut ri as *mut _ as *mut libc::rusage_info_t,
            );
            if rc == 0 {
                return Some(ri.ri_diskio_byteswritten);
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        read("/proc/self/io").and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("write_bytes:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
    }
}

/// Peak resident set size of this process in bytes.
pub fn max_rss() -> u64 {
    // SAFETY: getrusage fills a zeroed plain-data struct.
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let v = ru.ru_maxrss as u64;
        if cfg!(target_os = "macos") {
            v
        } else {
            v * 1024
        }
    }
}

/// Size of a file or the sum of a directory tree (allocated length).
pub fn path_size(p: &Path) -> u64 {
    let Ok(md) = std::fs::metadata(p) else {
        return 0;
    };
    if md.is_file() {
        return md.len();
    }
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            n += path_size(&e.path());
        }
    }
    n
}
