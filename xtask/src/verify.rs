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
            &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
        ),
        cargo_step("test", &["test", "--workspace", "--all-targets"]),
    ];
    if !quick {
        let mut doc = cargo_step("doc", &["doc", "--workspace", "--no-deps"]);
        doc.envs.push(("RUSTDOCFLAGS", "-D warnings"));
        steps.push(doc);
        steps.push(cargo_step("deny", &["deny", "check"]));
        steps.push(self_step("layering", "layering")?);
        steps.push(self_step("ac-audit", "ac-audit")?);
    }
    Ok(steps)
}

/// Runs every step (a failure does not stop the rest) and returns the names of the steps that failed.
pub fn run_steps(root: &Path, steps: &[Step]) -> Vec<&'static str> {
    let mut failed = Vec::new();
    for step in steps {
        println!("==> {}", step.name);
        let status = Command::new(&step.program)
            .args(&step.args)
            .envs(step.envs.iter().copied())
            .current_dir(root)
            .status();
        if !status.is_ok_and(|s| s.success()) {
            failed.push(step.name);
        }
    }
    failed
}
