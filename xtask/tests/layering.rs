//! Layering check tests (AC-REL-01, AC-REL-02, AC-CMP-01, AC-CMP-09).

mod common;

use xtask::layering::{self, ALLOWED, XTASK};
use xtask::workspace_root;

/// Writes a two-crate workspace where `velme-syntax` depends on `dependency` and returns its violations.
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
        "crates/velme-syntax/Cargo.toml",
        &format!(
            "[package]\nname = \"velme-syntax\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[{section}]\n{dependency} = {{ path = \"../{dependency}\" }}\n"
        ),
    )?;
    common::write(&dir, "crates/velme-syntax/src/lib.rs", "")?;
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
    let violations = violations_for_syntax_depending_on("layering_syntax_runtime", "velme-runtime", false)
        .expect("fixture workspace");
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("`velme-syntax` -> `velme-runtime`"),
        "{violations:?}"
    );
}

#[test]
fn ac_rel_02_layering_accepts_syntax_to_diagnostics() {
    let violations = violations_for_syntax_depending_on("layering_syntax_diagnostics", "velme-diagnostics", false)
        .expect("fixture workspace");
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn ac_rel_02_layering_allows_test_support_only_as_dev_dependency() {
    let dev = violations_for_syntax_depending_on("layering_test_support_dev", "velme-test-support", true)
        .expect("fixture workspace");
    assert!(dev.is_empty(), "{dev:?}");
    let normal = violations_for_syntax_depending_on("layering_test_support_normal", "velme-test-support", false)
        .expect("fixture workspace");
    assert_eq!(normal.len(), 1, "{normal:?}");
}

#[test]
fn ac_rel_02_layering_rejects_http_client_outside_synth() {
    let json = r#"{"packages":[
        {"name":"velme-sema","dependencies":[{"name":"reqwest","kind":null}]},
        {"name":"xtask","dependencies":[{"name":"ureq","kind":null}]},
        {"name":"velme-synth","dependencies":[{"name":"reqwest","kind":null}]}]}"#;
    let violations = layering::check(&layering::packages_from_metadata(json).expect("valid metadata"));
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations[0].contains("`velme-sema` depends on HTTP client `reqwest`"),
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

/// AC-CMP-09 (D4): membership is evaluated in `velme-check` and invoked only by `velme-runtime`; no body backend has an
/// edge to `velme-check` (the real `cargo metadata`), and no other crate's source calls the membership walk, except
/// `velme-synth`'s interim filter of generated inputs, which D5 removes (`compiler/22` §7.5).
#[test]
fn ac_cmp_09_membership_stays_in_velme_check_and_no_backend_reaches_it() {
    let root = workspace_root().expect("workspace root");
    let json = layering::metadata_json(&root.join("Cargo.toml")).expect("cargo metadata succeeds");
    let packages = layering::packages_from_metadata(&json).expect("valid metadata");
    let deps = |name: &str| {
        &packages
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is in the workspace"))
            .deps
    };
    assert!(
        deps("velme-runtime").iter().any(|d| d.name == "velme-check" && !d.dev),
        "velme-runtime must depend on velme-check"
    );
    for backend in ["velme-interp", "velme-wasm"] {
        assert!(
            deps(backend).iter().all(|d| d.name != "velme-check"),
            "{backend} has an edge to velme-check"
        );
    }

    let mut found = Vec::new();
    let crates = root.join("crates");
    for entry in std::fs::read_dir(&crates).expect("crates directory") {
        let dir = entry.expect("a crate").path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        if name == "velme-check" || name == "velme-runtime" {
            continue;
        }
        let mut stack = vec![dir.join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a source file");
                    let shown = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .display()
                        .to_string()
                        .replace('\\', "/");
                    found.extend(membership_calls(&shown, &text));
                }
            }
        }
    }
    assert!(found.is_empty(), "membership invoked outside velme-runtime: {found:#?}");
}

/// The membership calls in the source `text` of the file `path` that AC-CMP-09 forbids: any walk through `inputs` or
/// `output`, and `admits` anywhere but `velme-synth`'s verification, where a comment within the lines above it must say
/// it lasts until D5.
fn membership_calls(path: &str, text: &str) -> Vec<String> {
    if !text.contains("Membership") {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let walk = [".inputs(", ".output("].iter().any(|call| line.contains(call)) && line.contains("membership");
        let admits = line.contains(".admits(")
            && !(path == "crates/velme-synth/src/verify.rs"
                && lines
                    .get(n.saturating_sub(15)..n)
                    .unwrap_or_default()
                    .iter()
                    .any(|l| l.trim_start().starts_with("//") && l.contains("Until D5")));
        if walk || admits {
            found.push(format!("{path}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}
