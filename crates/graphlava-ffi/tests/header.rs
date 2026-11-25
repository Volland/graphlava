//! Checks that the checked-in `include/graphlava.h` matches what cbindgen
//! generates from `src/lib.rs` (rule `api-ffi-abi`: the header is generated and
//! versioned).
//!
//! The test runs the `cbindgen` binary (from `$CBINDGEN`, `PATH` or
//! `~/.cargo/bin`). Without it the test prints a message and passes, unless
//! `GRAPHLAVA_REQUIRE_CBINDGEN=1` is set (use that in CI after
//! `cargo install cbindgen --locked`). Set `GRAPHLAVA_BLESS=1` to rewrite the
//! checked-in header instead of comparing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn find_cbindgen() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("CBINDGEN") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("cbindgen"));
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(Path::new(&home).join(".cargo/bin/cbindgen"));
    }
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

#[test]
fn header_is_up_to_date() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header = root.join("include/graphlava.h");
    let Some(cbindgen) = find_cbindgen() else {
        assert!(
            std::env::var_os("GRAPHLAVA_REQUIRE_CBINDGEN").is_none(),
            "cbindgen not found but GRAPHLAVA_REQUIRE_CBINDGEN is set"
        );
        eprintln!("skipping header check: cbindgen not found (cargo install cbindgen)");
        return;
    };
    let bless = std::env::var_os("GRAPHLAVA_BLESS").is_some();
    let target = if bless {
        header.clone()
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("graphlava-{}.h", std::process::id()))
    };
    let out = Command::new(&cbindgen)
        .current_dir(root)
        .args([
            "--config",
            "cbindgen.toml",
            "--crate",
            "graphlava-ffi",
            "--quiet",
            "--output",
        ])
        .arg(&target)
        .output()
        .expect("run cbindgen");
    assert!(
        out.status.success(),
        "cbindgen failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if bless {
        return;
    }
    let fresh = std::fs::read_to_string(&target).expect("read generated header");
    let _ = std::fs::remove_file(&target);
    let checked_in = std::fs::read_to_string(&header).expect("read include/graphlava.h");
    assert!(
        fresh == checked_in,
        "include/graphlava.h is stale; regenerate with \
         `GRAPHLAVA_BLESS=1 cargo test -p graphlava-ffi --test header`"
    );
}
