//! The core crates carry no provider SDK (`delivery/52` AC-REL-03, `compiler/22` R-SYNTH-04) and `velme-synth`'s public API
//! takes no artifact store (`compiler/20` AC-CMP-08).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::process::Command;

use velme_test_support::repo;

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// The crates of the workspace, by name.
fn crates() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo("crates"))
        .expect("crates directory")
        .filter_map(Result::ok)
        .filter(|e| e.path().join("Cargo.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The normal dependencies of `krate` with default features off, one name and version per line.
fn tree(krate: &str, no_default_features: bool) -> String {
    let mut command = cargo();
    command
        .args(["tree", "--offline", "-e", "normal", "--prefix", "none", "-p", krate])
        .current_dir(repo(""));
    if no_default_features {
        command.arg("--no-default-features");
    }
    let out = command.output().expect("cargo tree runs");
    assert!(
        out.status.success(),
        "{krate}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

fn has(tree: &str, name: &str) -> bool {
    tree.lines().any(|line| line.split_whitespace().next() == Some(name))
}

/// Every crate but the binary builds its dependency tree without `ureq` and `rustls` when its default features are off,
/// while `velme-cli` has them; `velme-synth` compiles that way (AC-REL-03).
#[test]
fn ac_rel_03_core_crates_have_no_provider_sdk_in_their_tree() {
    for krate in crates()
        .iter()
        .filter(|c| !matches!(c.as_str(), "velme-cli" | "velme-test-support"))
    {
        let deps = tree(krate, true);
        for sdk in ["ureq", "rustls"] {
            assert!(!has(&deps, sdk), "{krate} pulls in {sdk} without its default features");
        }
    }
    assert!(
        has(&tree("velme-cli", false), "ureq"),
        "the binary carries the HTTP client"
    );
    let check = cargo()
        .args([
            "check",
            "--offline",
            "--quiet",
            "-p",
            "velme-synth",
            "--no-default-features",
        ])
        .current_dir(repo(""))
        .output()
        .expect("cargo check runs");
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
}

/// Nothing in `velme-synth` names an artifact store or the runtime that owns it: the crate takes no `&dyn ArtifactStore`
/// and has no dependency edge outside the layering (AC-CMP-08, D-54; the edges themselves are `cargo xtask layering`).
#[test]
fn ac_cmp_08_velme_synth_takes_no_artifact_store() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(dir.join("src")).expect("src").filter_map(Result::ok) {
        let text = std::fs::read_to_string(entry.path()).expect("source");
        assert!(!text.contains("ArtifactStore"), "{}", entry.path().display());
        assert!(!text.contains("velme_runtime"), "{}", entry.path().display());
    }
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).expect("manifest");
    let dependencies = manifest.split("[dev-dependencies]").next().expect("a section");
    assert!(!dependencies.contains("velme-runtime"));
}
