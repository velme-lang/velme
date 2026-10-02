//! `cargo xtask gate`: the phase-gate steps `cargo xtask verify` leaves out (`delivery/51` R-QA-07, D-130). The ignored
//! release-mode `ac_cmp_07_*` and `ac_qa_07_*` tests hold the `delivery/51` §6 targets, `d_134_flip_criterion` measures
//! the test of the CLI's default backend (D-134), and each `ac_rdm_*` test runs [`RUNS`] times, each in a fresh process,
//! for `delivery/50` R-RDM-04.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::verify::{Step, TEST_PROVIDER, in_fresh_home};

/// How many times each `ac_rdm_*` test runs (`delivery/50` R-RDM-04).
pub const RUNS: usize = 20;

/// The crates with a `tests/perf.rs` of ignored target tests (D-130).
pub const PERF_CRATES: &[&str] = &["velme-sema", "velme-ir", "velme-runtime", "velme-cli"];

/// The WASM speed target, held to a ratio of two timings in one process (D-130): it runs alone in a process of its own,
/// so its heap starts clean.
pub const WASM_RATIO_TEST: &str = "ac_qa_07_wasm_runs_the_same_workload_in_at_most_half_the_interpreters_time";

/// A `cargo test --release` of the ignored target tests, the `perf` test target of `crates`, one test at a time.
fn perf_test(name: &'static str, crates: &[&str], filter: &[&str], one_test: bool) -> Step {
    let mut args = ["test", "--release", "--no-fail-fast"].map(str::to_owned).to_vec();
    for name in crates {
        args.extend(["-p".to_owned(), (*name).to_owned()]);
    }
    args.extend(["--test", "perf", "--"].map(str::to_owned));
    args.extend(filter.iter().map(|a| (*a).to_owned()));
    args.extend(["--ignored", "--nocapture", "--test-threads=1"].map(str::to_owned));
    Step {
        name,
        program: crate::cargo_program(),
        args,
        envs: Vec::new(),
        one_test,
    }
}

/// The target tests' steps: every `tests/perf.rs`, ignored tests included, in release, one test at a time so no timing
/// shares the machine with another, and every crate's even when an earlier one fails; then [`WASM_RATIO_TEST`] alone.
pub fn perf_steps() -> Vec<Step> {
    vec![
        perf_test("perf", PERF_CRATES, &["--skip", WASM_RATIO_TEST], false),
        perf_test(
            "perf-wasm-ratio",
            &["velme-runtime"],
            &["--exact", WASM_RATIO_TEST],
            true,
        ),
    ]
}

/// A test binary of the default suite: its executable and the directory of the crate it tests.
pub struct TestBinary {
    /// The compiled test harness.
    pub executable: PathBuf,
    /// The crate's directory, where `cargo test` would run it.
    pub dir: PathBuf,
}

/// The test binaries in `cargo test --message-format=json` output: one per compiled test target.
pub fn test_binaries(messages: &str) -> Result<Vec<TestBinary>> {
    let mut binaries = Vec::new();
    for line in messages.lines().filter(|l| l.starts_with('{')) {
        let message: Value = serde_json::from_str(line).context("parsing a cargo message")?;
        if message.pointer("/reason").and_then(Value::as_str) != Some("compiler-artifact")
            || message.pointer("/profile/test").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }
        let Some(executable) = message.pointer("/executable").and_then(Value::as_str) else {
            continue;
        };
        let manifest = message
            .pointer("/manifest_path")
            .and_then(Value::as_str)
            .context("an artifact without a manifest path")?;
        let dir = Path::new(manifest)
            .parent()
            .context("a manifest path without a parent")?;
        binaries.push(TestBinary {
            executable: PathBuf::from(executable),
            dir: dir.to_path_buf(),
        });
    }
    Ok(binaries)
}

/// The `ac_rdm_*` tests in a test binary's `--list` output, by their full path. Pass it `--list --ignored` output for the
/// ignored ones, which [`rdm`] leaves out.
pub fn rdm_tests(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .filter(|name| name.rsplit("::").next().is_some_and(|last| last.starts_with("ac_rdm_")))
        .map(str::to_owned)
        .collect()
}

/// Whether a test run's output reports exactly one test passed: `--exact` with a name it doesn't find passes with no
/// test run.
pub fn ran_one(stdout: &str) -> bool {
    stdout.contains(" 1 passed")
}

/// Runs `command` with the provider settings scrubbed and a fresh, empty home (AC-QA-02), as `verify` runs a step.
fn scrubbed(mut command: Command) -> Result<std::process::Output> {
    in_fresh_home(&mut command, Command::output).context("running a test binary")
}

/// Builds the default suite, then runs each `ac_rdm_*` test [`RUNS`] times, each in a fresh process, printing its pass
/// count. Whether every one passed every run.
pub fn rdm(root: &Path) -> Result<bool> {
    println!("==> ac_rdm (each test {RUNS} times, each in a fresh process)");
    let mut build = Command::new(crate::cargo_program());
    build
        .args([
            "test",
            "--workspace",
            "--tests",
            "--features",
            TEST_PROVIDER,
            "--no-run",
            "--message-format=json",
        ])
        .current_dir(root);
    let built = scrubbed(build)?;
    if !built.status.success() {
        bail!(
            "building the test suite failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
    }
    let (mut tests, mut all_passed) = (0, true);
    for binary in test_binaries(&String::from_utf8_lossy(&built.stdout))? {
        let mut list = Command::new(&binary.executable);
        list.args(["--list"]).current_dir(&binary.dir);
        let listed = scrubbed(list)?;
        let mut list_ignored = Command::new(&binary.executable);
        list_ignored.args(["--list", "--ignored"]).current_dir(&binary.dir);
        let ignored = rdm_tests(&String::from_utf8_lossy(&scrubbed(list_ignored)?.stdout));
        let name = binary
            .executable
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for test in rdm_tests(&String::from_utf8_lossy(&listed.stdout)) {
            if ignored.contains(&test) {
                println!("ac_rdm: skipped, ignored: {test} ({name})");
                continue;
            }
            tests += 1;
            let (mut passed, mut first_failure) = (0, None);
            for run_number in 1..=RUNS {
                let mut run = Command::new(&binary.executable);
                run.args(["--exact", &test, "--test-threads=1"])
                    .current_dir(&binary.dir);
                let out = scrubbed(run)?;
                let stdout = String::from_utf8_lossy(&out.stdout);
                if out.status.success() && ran_one(&stdout) {
                    passed += 1;
                } else if first_failure.is_none() {
                    first_failure = Some(format!(
                        "run {run_number}:\n{stdout}{}",
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
            }
            all_passed &= passed == RUNS;
            println!("ac_rdm: {passed}/{RUNS} {test} ({name})");
            if let Some(failure) = first_failure {
                println!("ac_rdm: the first failure of {test}, {failure}");
            }
        }
    }
    if tests == 0 {
        bail!("no `ac_rdm_*` test found");
    }
    Ok(all_passed)
}
