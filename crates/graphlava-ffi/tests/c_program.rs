//! Compiles each program in `tests/c/` against the checked-in header, links it
//! with the staticlib cargo built for this test run, and runs it. Skips (with a
//! message) when no C compiler is available or the platform is not Unix.

use std::path::{Path, PathBuf};
use std::process::Command;

fn have_cc(cc: &str) -> bool {
    Command::new(cc)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// System libraries a Rust staticlib needs (`rustc --print native-static-libs`).
fn native_libs() -> &'static [&'static str] {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        &["-lSystem", "-lc", "-lm", "-liconv"]
    } else {
        &[
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]
    }
}

/// `libgraphlava_ffi.a` sits next to this test binary in `target/<profile>/deps`.
/// `cargo test` builds the library only as an rlib, so the staticlib may
/// not exist yet: build it once with the same profile when it is missing.
fn staticlib() -> Option<PathBuf> {
    // Tests run in parallel; build the staticlib at most once.
    static LIB: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    LIB.get_or_init(find_or_build_staticlib).clone()
}

fn find_or_build_staticlib() -> Option<PathBuf> {
    // The test binary sits in `target/<profile>/deps/` or, with cargo's newer
    // build-dir layout, deeper under `target/<profile>/build/`; the staticlib
    // is in `target/<profile>/`. Search every ancestor.
    let exe = std::env::current_exe().ok()?;
    let find = || {
        exe.ancestors()
            .map(|d| d.join("libgraphlava_ffi.a"))
            .find(|p| p.exists())
    };
    if let Some(p) = find() {
        return Some(p);
    }
    let release = exe
        .ancestors()
        .any(|d| d.file_name().is_some_and(|n| n == "release"));
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "graphlava-ffi", "--lib"]);
    if release {
        cmd.arg("--release");
    }
    match cmd.output() {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!(
                "building libgraphlava_ffi.a failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return None;
        }
        Err(e) => {
            eprintln!("could not run cargo to build libgraphlava_ffi.a: {e}");
            return None;
        }
    }
    find()
}

/// Compiles `tests/c/<name>.c` and runs it on a fresh file. Extra compiler
/// flags come from `GRAPHLAVA_C_FLAGS` (CI sets
/// `-fsanitize=address,leak -fno-omit-frame-pointer`).
fn build_and_run(name: &str) {
    if !cfg!(unix) {
        eprintln!("skipping C program test: only run on Unix hosts");
        return;
    }
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    if !have_cc(&cc) {
        eprintln!("skipping C program test: no C compiler `{cc}` found");
        return;
    }
    let Some(lib) = staticlib() else {
        panic!("libgraphlava_ffi.a not found next to the test binary");
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let exe = tmp.join(format!("graphlava_c_{name}"));
    let flags: Vec<String> = std::env::var("GRAPHLAVA_C_FLAGS")
        .map(|f| f.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let out = Command::new(&cc)
        .args(["-std=c99", "-Wall", "-Wextra", "-Werror"])
        .args(&flags)
        .arg("-o")
        .arg(&exe)
        .arg(root.join(format!("tests/c/{name}.c")))
        .arg("-I")
        .arg(root.join("include"))
        .arg(&lib)
        .args(native_libs())
        .output()
        .expect("run the C compiler");
    assert!(
        out.status.success(),
        "C compile failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let db = tmp.join(format!("c-{name}-{}.glv", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let run = Command::new(&exe)
        .arg(&db)
        .output()
        .expect("run the C program");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "C program failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    assert!(!stderr.contains("Sanitizer"), "sanitizer report:\n{stderr}");
    let _ = std::fs::remove_file(&db);
}

#[test]
fn c_program_links_and_runs() {
    build_and_run("smoke");
}

// @tg: verifies:: [[21-e2e-tests#C FFI lifecycle]] {level: e2e}
#[test]
fn c_agent_memory_session() {
    build_and_run("session");
}

// @tg: verifies:: [[openspec:mobile-bindings#Complete C FFI#C lifecycle under sanitizers]] {level: e2e}
#[test]
fn c_lifecycle_through_every_group() {
    build_and_run("lifecycle");
}

// @tg: verifies:: [[openspec:mobile-bindings#FFI misuse safety#Use after close]] {level: e2e}
// @tg: verifies:: [[openspec:mobile-bindings#Complete C FFI#Header version mismatch]] {level: unit}
#[test]
fn c_misuse_returns_error_statuses() {
    build_and_run("misuse");
}

/// The snippets of `README.md`.
#[test]
fn c_readme_snippets() {
    build_and_run("readme");
}
