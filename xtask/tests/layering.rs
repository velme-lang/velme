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

/// AC-CMP-09 (D4 runtime, D5 probe): membership is evaluated in `velme-check` and its walk invoked only by
/// `velme-runtime`; no body backend has an edge to `velme-check` and `velme-check` none to `velme-synth` (the real
/// `cargo metadata`); no other crate's source calls the walk; and the draws and the probe live in `velme-synth`, which
/// `velme check` calls during analysis, before any provider exists (`compiler/22` R-SYNTH-65, D-164).
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
    assert!(
        deps("velme-check").iter().all(|d| d.name != "velme-synth"),
        "velme-check has an edge to velme-synth"
    );
    let source = |path: &str| std::fs::read_to_string(root.join(path)).expect("a source file");
    let probe = source("crates/velme-synth/src/probe.rs");
    assert!(
        probe.contains("pub fn probe(") && probe.contains("generator.admitted("),
        "the probe lives in velme-synth and evaluates predicates through velme-check"
    );
    assert!(
        source("crates/velme-synth/src/generate.rs").contains("membership.member_within("),
        "generated values are held to their domains by velme-check's predicate"
    );
    // The CLI probes while it analyzes, which constructs no provider; the providers come later, in `synth.rs`.
    let analyze = source("crates/velme-cli/src/main.rs");
    let analyze = analyze
        .split("fn analyze(")
        .nth(1)
        .and_then(|rest| rest.split("\nfn ").next())
        .expect("velme-cli has `analyze`");
    assert!(
        analyze.contains("velme_synth::probe("),
        "`velme check` probes during analysis"
    );
    assert!(!analyze.contains("provider"), "analysis constructs no provider");

    let mut found = Vec::new();
    let mut scanned = std::collections::BTreeSet::new();
    let mut seen = Vec::new();
    let crates = root.join("crates");
    for entry in std::fs::read_dir(&crates).expect("crates directory") {
        let dir = entry.expect("a crate").path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let mut stack = vec![dir.join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    scanned.insert(name.clone());
                    let text = std::fs::read_to_string(&path).expect("a source file");
                    let shown = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .display()
                        .to_string()
                        .replace('\\', "/");
                    for call in membership_calls(&text) {
                        seen.push((name.clone(), call.kind));
                        if !call.kind.allowed_in(&name) {
                            found.push(format!("{shown}:{}: {:?}", call.line, call.kind));
                        }
                    }
                }
            }
        }
    }
    // A moved tree scans nothing and would pass: the crates this rule is about must have been read, and the calls they
    // are allowed must have been seen.
    for expected in [
        "velme-check",
        "velme-cli",
        "velme-interp",
        "velme-runtime",
        "velme-synth",
        "velme-wasm",
    ] {
        assert!(scanned.contains(expected), "{expected} was not scanned: {scanned:?}");
    }
    for (krate, kind) in [
        ("velme-runtime", CallKind::Walk),
        ("velme-runtime", CallKind::New),
        ("velme-synth", CallKind::Member),
        ("velme-synth", CallKind::New),
        ("velme-check", CallKind::Member),
    ] {
        assert!(
            seen.iter().any(|(name, k)| name == krate && *k == kind),
            "{krate} should show {kind:?}: the scan no longer sees what it is looking for"
        );
    }
    assert!(found.is_empty(), "membership invoked outside its crates: {found:#?}");
}

/// AC-CMP-09, the layering finding of D5: the scan sees a walk across line breaks and on any receiver, and each call in
/// a crate it isn't allowed in, and isn't fooled by a comment or by a process's `.output()`.
#[test]
fn ac_cmp_09_the_membership_scan_catches_what_it_should() {
    let lines = |text: &str| -> Vec<(usize, CallKind)> {
        membership_calls(text).into_iter().map(|c| (c.line, c.kind)).collect()
    };
    let walk =
        "use velme_check::Membership;\nfn f(m: &Membership) {\n    let w = m\n        .inputs(\n        t, a, b);\n}\n";
    assert_eq!(lines(walk), [(4, CallKind::Walk)]);
    assert!(!CallKind::Walk.allowed_in("velme-synth"));
    assert!(CallKind::Walk.allowed_in("velme-runtime"));
    let output = "// Membership\nfn f(tracker: &velme_check::Membership) { tracker.output (t, v, b); }\n";
    assert_eq!(lines(output), [(2, CallKind::Walk)]);
    let member = "fn f(x: &Membership) { x\n.member(id, v); x.member_within(id, v, 1); }";
    assert_eq!(lines(member), [(2, CallKind::Member), (2, CallKind::Member)]);
    assert!(!CallKind::Member.allowed_in("velme-runtime"));
    assert!(CallKind::Member.allowed_in("velme-synth"));
    let new = "let m = velme_check::Membership::new(p, s);";
    assert_eq!(lines(new), [(1, CallKind::New)]);
    assert!(!CallKind::New.allowed_in("velme-cli"));
    assert!(CallKind::New.allowed_in("velme-runtime"));
    // Not membership: a process's output, a file that never names it, and a comment.
    assert!(lines("use velme_check::Membership;\nlet o = command.output();\n").is_empty());
    assert!(lines("reader.output(&a, b);").is_empty());
    assert!(lines("// Membership: m.inputs(t, a, b)\n/// m.member(x, y)\n").is_empty());
}

/// What a source file does with `velme_check::Membership`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallKind {
    /// The walk over a value, `.inputs(…)` or `.output(…)`: `velme-runtime`'s alone (AC-CMP-09).
    Walk,
    /// One domain's predicate, `.member(…)` or `.member_within(…)`: the generator and probe in `velme-synth`, the
    /// source-literal pass in `velme-check` (R-CHK-15).
    Member,
    /// `Membership::new`: wherever one of the others is allowed. The runtime needs one for its walk.
    New,
}

impl CallKind {
    fn allowed_in(self, krate: &str) -> bool {
        match self {
            CallKind::Walk => matches!(krate, "velme-check" | "velme-runtime"),
            CallKind::Member => matches!(krate, "velme-check" | "velme-synth"),
            CallKind::New => matches!(krate, "velme-check" | "velme-runtime" | "velme-synth"),
        }
    }
}

struct MembershipCall {
    kind: CallKind,
    /// The 1-based line of the call's name.
    line: usize,
}

/// The membership calls in the source `text`, comments aside and spacing and line breaks ignored, on any receiver. A
/// walk's name isn't unique (`command.output()`, `reader.output(…)`), so it counts in a file that names `Membership` and
/// only with arguments.
fn membership_calls(text: &str) -> Vec<MembershipCall> {
    // The code squeezed of whitespace, and the line of each of its bytes.
    let mut code = String::new();
    let mut lines = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split("//").next().unwrap_or_default();
        for c in line.chars().filter(|c| !c.is_whitespace()) {
            code.push(c);
            lines.resize(code.len(), n + 1);
        }
    }
    let names = code.contains("Membership") || code.contains("membership");
    let mut found = Vec::new();
    let mut add = |pattern: &str, kind: CallKind, wanted: &dyn Fn(&str) -> bool| {
        for (at, _) in code.match_indices(pattern) {
            if wanted(code.get(at + pattern.len()..).unwrap_or_default()) {
                let line = lines.get(at + 1).copied().unwrap_or(0);
                found.push(MembershipCall { kind, line });
            }
        }
    };
    if names {
        let with_arguments = |rest: &str| !rest.starts_with(')');
        add(".inputs(", CallKind::Walk, &with_arguments);
        add(".output(", CallKind::Walk, &with_arguments);
    }
    add(".member(", CallKind::Member, &|_| true);
    add(".member_within(", CallKind::Member, &|_| true);
    add("Membership::new(", CallKind::New, &|_| true);
    found.sort_by_key(|c| c.line);
    found
}
