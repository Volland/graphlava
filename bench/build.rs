//! Records the compiler and flags the harness was built with, so every result
//! file names them ([[17-benchmarks#Harness and method]]).

use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let v = Command::new(&rustc)
        .arg("-vV")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let line = |k: &str| {
        v.lines()
            .find_map(|l| l.strip_prefix(k).map(|s| s.trim().to_string()))
            .unwrap_or_default()
    };
    println!("cargo:rustc-env=MB_RUSTC_RELEASE={}", line("release:"));
    println!("cargo:rustc-env=MB_RUSTC_COMMIT={}", line("commit-hash:"));
    println!("cargo:rustc-env=MB_RUSTC_LLVM={}", line("LLVM version:"));
    println!(
        "cargo:rustc-env=MB_RUSTC_HOST={}",
        line("host:")
    );
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS")
        .unwrap_or_default()
        .replace('\u{1f}', " ");
    println!("cargo:rustc-env=MB_RUSTFLAGS={flags}");
    for k in ["PROFILE", "OPT_LEVEL", "DEBUG", "TARGET"] {
        println!(
            "cargo:rustc-env=MB_{k}={}",
            std::env::var(k).unwrap_or_default()
        );
    }
    let mut feats: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| k.strip_prefix("CARGO_FEATURE_").map(|s| s.to_lowercase()))
        .collect();
    feats.sort();
    println!("cargo:rustc-env=MB_FEATURES={}", feats.join(","));
    println!("cargo:rerun-if-env-changed=RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
}
