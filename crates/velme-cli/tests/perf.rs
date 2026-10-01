//! The `delivery/51` §6 targets of the `velme` binary, CLI start to exit (D-130, D-131): `velme check` on the seeded
//! generator's 1 000-line program, and `velme run --locked` on `find_badge`. Ignored; `cargo xtask gate` runs them in
//! release.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use velme_test_support::workload::{assert_release, best_of, find_badge, find_badge_run, source, user_cache, velme};

/// A fresh scratch directory `name` for one test.
fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf").join(name)
}

/// `velme check` on the generated 1 000-line program takes under 100 ms, the best of 10 (AC-QA-07, `delivery/51` §6).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_check_of_a_1000_line_file_is_under_100_ms() {
    assert_release("ac_qa_07");
    let dir = scratch("check");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    fs::write(dir.join("gen.velme"), source(1, 1000)).expect("written");
    let exe = Path::new(env!("CARGO_BIN_EXE_velme"));
    let args = ["check".to_owned(), "gen.velme".to_owned()];
    let took = best_of(10, || velme(exe, &dir, &args));
    println!("velme check, 1 000 lines: {took:.2?}");
    assert!(took < Duration::from_millis(100), "{took:?}");
}

/// `velme run --locked` on `find_badge`, a cache hit, takes under 50 ms on the interpreter and on `auto`, the best of 10
/// (AC-QA-07, `delivery/51` §6).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_run_locked_of_find_badge_is_under_50_ms_on_interp_and_auto() {
    assert_release("ac_qa_07");
    let dir = find_badge(&scratch("find_badge"));
    let exe = Path::new(env!("CARGO_BIN_EXE_velme"));
    for backend in ["interp", "auto"] {
        // The first run fills the module cache `auto` reads from (R-SBX-20); `interp` leaves it empty.
        velme(exe, &dir, &find_badge_run(backend));
        let filled = fs::read_dir(user_cache(&dir).join("velme").join("wasm")).is_ok_and(|mut d| d.next().is_some());
        assert_eq!(
            filled,
            backend == "auto",
            "{backend}: the module cache is filled after a run"
        );
        let took = best_of(10, || velme(exe, &dir, &find_badge_run(backend)));
        println!("velme run --locked, find_badge, {backend}: {took:.2?}");
        assert!(took < Duration::from_millis(50), "{backend}: {took:?}");
    }
}
