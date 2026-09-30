//! Release-feature check (`tooling/41` R-SEC-07 area, `compiler/22` R-SYNTH-44, D-98): the test-only Cargo features never
//! reach `velme-cli`'s normal dependencies, so no release build carries a test endpoint.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The feature that lets a provider talk to a mock endpoint (`velme-synth`, R-SYNTH-44). `velme-test-support` turns it on;
/// nothing in `velme-cli`'s normal dependencies may.
pub const TEST_ENDPOINT: &str = "test-endpoint";

/// The lines of a `cargo tree -e features` listing that name [`TEST_ENDPOINT`].
pub fn offending_lines(tree: &str) -> Vec<&str> {
    tree.lines().filter(|line| line.contains(TEST_ENDPOINT)).collect()
}

/// Runs `cargo tree -e features,normal -p velme-cli` in `root` and returns the lines that enable [`TEST_ENDPOINT`].
pub fn check(root: &Path) -> Result<Vec<String>> {
    let output = Command::new(crate::cargo_program())
        .args(["tree", "-e", "features,normal", "-p", "velme-cli", "--locked"])
        .current_dir(root)
        .output()
        .context("running `cargo tree`")?;
    if !output.status.success() {
        bail!(
            "`cargo tree` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let tree = String::from_utf8_lossy(&output.stdout);
    Ok(offending_lines(&tree).into_iter().map(str::to_owned).collect())
}
