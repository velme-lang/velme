//! `cargo xtask verify`: the one local gate (`delivery/51` §4, R-QA-07, AC-QA-01).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// One gate step: a command run from the repository root.
pub struct Step {
    /// Name shown in the summary.
    pub name: &'static str,
    /// Program to run.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub envs: Vec<(&'static str, &'static str)>,
    /// Whether the step runs one test with `--exact`, which passes with none found, so it must also report one passed.
    pub one_test: bool,
}

/// The `velme-cli` feature that adds the `scripted` provider (`tooling/40` §5.2, D-94): on for the gate's tests, never for
/// a release build.
pub(crate) const TEST_PROVIDER: &str = "velme-cli/test-provider";

fn cargo_step(name: &'static str, args: &[&str]) -> Step {
    Step {
        name,
        program: crate::cargo_program(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        envs: Vec::new(),
        one_test: false,
    }
}

/// This binary re-run with a subcommand and its arguments, so in-process checks report like any other step.
fn self_step(name: &'static str, args: &[&str]) -> Result<Step> {
    let exe = std::env::current_exe().context("locating the xtask binary")?;
    Ok(Step {
        name,
        program: exe.to_string_lossy().into_owned(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        envs: Vec::new(),
        one_test: false,
    })
}

/// The gate steps in run order; `quick` is the inner loop (fmt, clippy, tests).
pub fn steps(quick: bool) -> Result<Vec<Step>> {
    let mut steps = vec![
        cargo_step("fmt", &["fmt", "--all", "--check"]),
        cargo_step(
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--features",
                TEST_PROVIDER,
                "--",
                "-D",
                "warnings",
            ],
        ),
        // What a user gets from `cargo clippy -p velme-cli`: the default features, where `test-provider` is off and code
        // behind `cfg(feature = "test-provider")` is not seen (D-94).
        cargo_step(
            "clippy-cli",
            &["clippy", "-p", "velme-cli", "--all-targets", "--", "-D", "warnings"],
        ),
        cargo_step(
            "test",
            &["test", "--workspace", "--all-targets", "--features", TEST_PROVIDER],
        ),
    ];
    if !quick {
        let mut doc = cargo_step("doc", &["doc", "--workspace", "--no-deps"]);
        doc.envs.push(("RUSTDOCFLAGS", "-D warnings"));
        steps.push(doc);
        steps.push(cargo_step("deny", &["deny", "check"]));
        steps.push(self_step("layering", &["layering"])?);
        steps.push(self_step("features", &["features"])?);
        // Every criterion needs a test (D-139).
        steps.push(self_step("ac-audit", &["ac-audit", "--strict"])?);
    }
    Ok(steps)
}

// The provider settings no step sees (AC-QA-02), kept with the release-mode targets' copy in one file.
#[path = "../../crates/velme-test-support/src/scrub.rs"]
mod scrub;
pub use scrub::scrub_provider_env;

/// Numbers the homes of [`run_steps`].
static HOMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Runs every step (a failure does not stop the rest) and returns the names of the steps that failed. Each step gets a
/// fresh, empty home of its own, removed afterwards, so no step sees what an earlier one left in it.
pub fn run_steps(root: &Path, steps: &[Step]) -> Vec<&'static str> {
    let mut failed = Vec::new();
    for step in steps {
        println!("==> {}", step.name);
        // Unique within the process too, so two runs at once (tests) never share one.
        let n = HOMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let home = std::env::temp_dir().join(format!("velme-gate-home-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::create_dir_all(&home);
        let mut command = Command::new(&step.program);
        command
            .args(&step.args)
            .envs(step.envs.iter().copied())
            .current_dir(root);
        scrub_provider_env(&mut command, &home);
        let passed = if step.one_test {
            // Captured to count what ran, then shown as it would have been.
            command.output().is_ok_and(|out| {
                print!("{}", String::from_utf8_lossy(&out.stdout));
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
                out.status.success() && crate::gate::ran_one(&String::from_utf8_lossy(&out.stdout))
            })
        } else {
            command.status().is_ok_and(|s| s.success())
        };
        let _ = std::fs::remove_dir_all(&home);
        if !passed {
            failed.push(step.name);
        }
    }
    failed
}
