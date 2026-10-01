//! The `delivery/51` §6 targets of the `velme` binary, CLI start to exit (D-130, D-131): `velme check` on the seeded
//! generator's 1 000-line program, and `velme run --locked` on `find_badge`; and D-134's test of the CLI's default
//! backend. Ignored; `cargo xtask gate` runs them in release.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use velme_test_support::workload::{
    assert_release, best_of, find_badge, find_badge_run, fixture_project, paired, source, user_cache, velme,
    velme_stderr,
};

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

/// The examples D-134 (1) holds to within 3 ms of the interpreter: every example but [`LEVEL_SUMMARY`].
const SMALL: [&str; 6] = [
    "add",
    "double_then_add_one",
    "find_badge",
    "hello",
    "order_total",
    "player_summary",
];

/// The example D-134 (1) holds to no slower than the interpreter.
const LEVEL_SUMMARY: &str = "level_summary";

/// How many interleaved pairs each D-134 timing takes its median over.
const PAIRS: usize = 31;

/// The median of `sorted`, which is not empty.
fn median(sorted: &[f64]) -> f64 {
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    }
}

/// D-134 (1), the test of the CLI's default backend, with a warm module cache: `velme test` under `auto` is within 3 ms
/// of the interpreter on every small example and no slower on `level_summary`, by the median of [`PAIRS`] interleaved
/// pairs, and `velme run --locked` of `find_badge` under `auto` takes under 50 ms, by the median of [`PAIRS`] runs. It
/// prints the table; it fails only on a platform whose default is `auto` (D-134 (2), (3)), and there the default is
/// found from a `velme test -v` with no `--backend`, which prints its `module cache:` line only if it made the WASM
/// backend (D-137), with or without a disk cache.
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn d_134_flip_criterion() {
    assert_release("d_134");
    let exe = Path::new(env!("CARGO_BIN_EXE_velme"));
    let test = |name: &str, backend: &str| {
        let args = ["test", &format!("{name}.velme"), "--backend", backend];
        args.map(str::to_owned).to_vec()
    };
    let mut failed = Vec::new();
    println!(
        "D-134 (1), {} pairs, medians in ms: example | interp | auto | auto - interp | limit",
        PAIRS
    );
    for name in SMALL.into_iter().chain([LEVEL_SUMMARY]) {
        let dir = fixture_project(name, &scratch(&format!("d_134_{name}")));
        // The first run fills the module cache, so every timed one finds it warm (R-SBX-20).
        velme(exe, &dir, &test(name, "auto"));
        let pairs = paired(
            PAIRS,
            || velme(exe, &dir, &test(name, "interp")),
            || velme(exe, &dir, &test(name, "auto")),
        );
        let ms = |pick: &dyn Fn(&(f64, f64)) -> f64| {
            let mut v: Vec<f64> = pairs.iter().map(|p| pick(p) * 1e3).collect();
            v.sort_by(f64::total_cmp);
            median(&v)
        };
        let (interp, auto, delta) = (ms(&|p| p.0), ms(&|p| p.1), ms(&|p| p.1 - p.0));
        let limit = if name == LEVEL_SUMMARY { 0.0 } else { 3.0 };
        let pass = delta <= limit;
        println!(
            "velme test {name} | {interp:.2} | {auto:.2} | {delta:+.2} | {limit:+.1} | {}",
            if pass { "pass" } else { "FAIL" }
        );
        if !pass {
            failed.push(format!(
                "velme test {name}: auto - interp {delta:+.2} ms > {limit:+.1} ms"
            ));
        }
    }
    let dir = find_badge(&scratch("d_134_locked"));
    velme(exe, &dir, &find_badge_run("auto"));
    let mut runs: Vec<f64> = (0..PAIRS)
        .map(|_| {
            let start = Instant::now();
            velme(exe, &dir, &find_badge_run("auto"));
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    let locked = median(&runs);
    let pass = locked < 50.0;
    println!(
        "velme run --locked find_badge, auto | {locked:.2} | < 50 | {}",
        if pass { "pass" } else { "FAIL" }
    );
    if !pass {
        failed.push(format!("velme run --locked find_badge on auto: {locked:.2} ms"));
    }
    // The default backend: only a command that made the WASM backend prints the module cache's line.
    let dir = fixture_project("hello", &scratch("d_134_default"));
    let args = ["test", "hello.velme", "-v"].map(str::to_owned);
    let auto = velme_stderr(exe, &dir, &args)
        .lines()
        .any(|l| l.starts_with("module cache:"));
    let default = if auto { "auto" } else { "interp" };
    println!(
        "D-134 (1) {}; the default backend here is {default}",
        if failed.is_empty() { "passes" } else { "fails" }
    );
    assert!(
        !auto || failed.is_empty(),
        "the default is auto, but D-134 (1) fails: {failed:#?}"
    );
}
