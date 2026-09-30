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
const TEST_PROVIDER: &str = "velme-cli/test-provider";

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
        steps.push(self_step("ac-audit", "ac-audit")?);
    }
    Ok(steps)
}

/// The provider settings the gate never lets into a step, so the default suite runs as it does with no key, no provider and
/// no live tests (`delivery/51` AC-QA-02, D-13); every variable ending in `_API_KEY` goes too.
const PROVIDER_ENV: [&str; 5] = [
    "VELME_MODEL",
    "VELME_EXTERNAL_COMMAND",
    "VELME_SYNTH_RECORD",
    "VELME_SYNTH_SCRIPT",
    "VELME_LIVE_LLM",
];

/// Removes from `command`'s environment everything a provider could be reached with, and points the user-level config
/// (`$XDG_CONFIG_HOME`, `%APPDATA%`, and `HOME` for the platform equivalent) at `home`, an empty directory, so a step never
/// reads the developer's own settings (AC-QA-02). The toolchain keeps the homes it had.
pub fn scrub_provider_env(command: &mut Command, home: &Path) {
    let old_home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if std::env::var_os(var).is_none()
            && let Some(old) = &old_home
        {
            command.env(var, old.join(dir));
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home);
    for name in PROVIDER_ENV {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().to_ascii_uppercase().ends_with("_API_KEY") {
            command.env_remove(name);
        }
    }
}

/// Runs every step (a failure does not stop the rest) and returns the names of the steps that failed.
pub fn run_steps(root: &Path, steps: &[Step]) -> Vec<&'static str> {
    let mut failed = Vec::new();
    let home = std::env::temp_dir().join(format!("velme-gate-home-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    for step in steps {
        println!("==> {}", step.name);
        let mut command = Command::new(&step.program);
        command
            .args(&step.args)
            .envs(step.envs.iter().copied())
            .current_dir(root);
        scrub_provider_env(&mut command, &home);
        let status = command.status();
        if !status.is_ok_and(|s| s.success()) {
            failed.push(step.name);
        }
    }
    failed
}
