//! Layering check tests (AC-REL-01, AC-REL-02, AC-CMP-01).

mod common;

use xtask::layering::{self, ALLOWED, XTASK};
use xtask::workspace_root;

/// Writes a two-crate workspace where `thela-syntax` depends on `dependency` and returns its violations.
fn violations_for_syntax_depending_on(test: &str, dependency: &str, dev: bool) -> anyhow::Result<Vec<String>> {
    let dir = common::fresh_dir(test)?;
    common::write(
        &dir,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n",
    )?;
    let section = if dev { "dev-dependencies" } else { "dependencies" };
    common::write(
        &dir,
        "crates/thela-syntax/Cargo.toml",
        &format!(
            "[package]\nname = \"thela-syntax\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[{section}]\n{dependency} = {{ path = \"../{dependency}\" }}\n"
        ),
    )?;
    common::write(&dir, "crates/thela-syntax/src/lib.rs", "")?;
    common::write(
        &dir,
        &format!("crates/{dependency}/Cargo.toml"),
        &format!("[package]\nname = \"{dependency}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )?;
    common::write(&dir, &format!("crates/{dependency}/src/lib.rs"), "")?;
    layering::check_manifest(&dir.join("Cargo.toml"))
}

// Also covers AC-CMP-01.
#[test]
fn ac_rel_02_layering_fails_on_syntax_to_runtime() {
    let violations = violations_for_syntax_depending_on("layering_syntax_runtime", "thela-runtime", false)
        .expect("fixture workspace");
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("`thela-syntax` -> `thela-runtime`"),
        "{violations:?}"
    );
}

#[test]
fn ac_rel_02_layering_accepts_syntax_to_diagnostics() {
    let violations = violations_for_syntax_depending_on("layering_syntax_diagnostics", "thela-diagnostics", false)
        .expect("fixture workspace");
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn ac_rel_02_layering_allows_test_support_only_as_dev_dependency() {
    let dev = violations_for_syntax_depending_on("layering_test_support_dev", "thela-test-support", true)
        .expect("fixture workspace");
    assert!(dev.is_empty(), "{dev:?}");
    let normal = violations_for_syntax_depending_on("layering_test_support_normal", "thela-test-support", false)
        .expect("fixture workspace");
    assert_eq!(normal.len(), 1, "{normal:?}");
}

#[test]
fn ac_rel_02_layering_rejects_http_client_outside_synth() {
    let json = r#"{"packages":[
        {"name":"thela-sema","dependencies":[{"name":"reqwest","kind":null}]},
        {"name":"xtask","dependencies":[{"name":"ureq","kind":null}]},
        {"name":"thela-synth","dependencies":[{"name":"reqwest","kind":null}]}]}"#;
    let violations = layering::check(&layering::packages_from_metadata(json).expect("valid metadata"));
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations[0].contains("`thela-sema` depends on HTTP client `reqwest`"),
        "{violations:?}"
    );
}

#[test]
fn ac_rel_01_workspace_has_exactly_the_d15_crates_and_xtask() {
    let root = workspace_root().expect("workspace root");
    let mut expected: Vec<&str> = ALLOWED.iter().map(|(name, _)| *name).collect();
    expected.push(XTASK);
    expected.sort_unstable();
    let json = layering::metadata_json(&root.join("Cargo.toml")).expect("cargo metadata succeeds");
    let mut actual: Vec<String> = layering::packages_from_metadata(&json)
        .expect("valid metadata")
        .into_iter()
        .map(|p| p.name)
        .collect();
    actual.sort_unstable();
    assert_eq!(actual, expected);
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).expect("rust-toolchain.toml exists");
    assert!(
        toolchain.contains("channel = \"1."),
        "toolchain must be pinned: {toolchain}"
    );
}

#[test]
fn ac_cmp_01_the_real_workspace_passes_the_layering_check() {
    let root = workspace_root().expect("workspace root");
    let violations = layering::check_manifest(&root.join("Cargo.toml")).expect("cargo metadata succeeds");
    assert!(violations.is_empty(), "{violations:?}");
}
