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
    }
}

/// This binary re-run with a subcommand, so in-process checks report like any other step.
fn self_step(name: &'static str, subcommand: &str) -> Result<Step> {
    let exe = std::env::current_exe().context("locating the xtask binary")?;
    Ok(Step {
        name,
        program: exe.to_string_lossy().into_owned(),
        args: vec![subcommand.to_owned()],
        envs: Vec::new(),
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
        steps.push(self_step("layering", "layering")?);
        steps.push(self_step("features", "features")?);
        steps.push(self_step("ac-audit", "ac-audit")?);
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
        let status = command.status();
        let _ = std::fs::remove_dir_all(&home);
        if !status.is_ok_and(|s| s.success()) {
            failed.push(step.name);
        }
    }
    failed
}
