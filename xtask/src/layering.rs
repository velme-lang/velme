//! Crate-graph check for INV-9 (R-REL-03, R-CMP-01, AC-REL-02, AC-CMP-01).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Every library crate; the allow-list of `velme-cli` and `velme-test-support`.
const LIBRARIES: &[&str] = &[
    "velme-diagnostics",
    "velme-syntax",
    "velme-builtins",
    "velme-sema",
    "velme-ir",
    "velme-check",
    "velme-interp",
    "velme-synth",
    "velme-wasm",
    "velme-runtime",
];

/// The workspace crate each crate may depend on: the table in `compiler/20` §2 (D-15).
pub const ALLOWED: &[(&str, &[&str])] = &[
    ("velme-diagnostics", &[]),
    ("velme-syntax", &["velme-diagnostics"]),
    ("velme-builtins", &["velme-diagnostics"]),
    ("velme-sema", &["velme-syntax", "velme-builtins", "velme-diagnostics"]),
    ("velme-ir", &["velme-sema", "velme-builtins", "velme-diagnostics"]),
    ("velme-check", &["velme-ir", "velme-sema", "velme-diagnostics"]),
    ("velme-interp", &["velme-ir", "velme-builtins", "velme-diagnostics"]),
    (
        "velme-synth",
        &[
            "velme-ir",
            "velme-check",
            "velme-interp",
            "velme-sema",
            "velme-diagnostics",
        ],
    ),
    ("velme-wasm", &["velme-ir", "velme-builtins", "velme-diagnostics"]),
    (
        "velme-runtime",
        &[
            "velme-ir",
            "velme-check",
            "velme-interp",
            "velme-wasm",
            "velme-synth",
            "velme-builtins",
            "velme-diagnostics",
        ],
    ),
    ("velme-cli", LIBRARIES),
    // Dev-dependency only; see `TEST_SUPPORT` below.
    ("velme-test-support", LIBRARIES),
];

/// The workspace crates `name` may depend on, or `None` if it is not a D-15 crate.
pub fn allowed_deps(name: &str) -> Option<&'static [&'static str]> {
    ALLOWED.iter().find(|(c, _)| *c == name).map(|(_, may)| *may)
}

/// May only be a dev-dependency (`delivery/52` §3).
pub const TEST_SUPPORT: &str = "velme-test-support";
/// The automation crate: in the workspace, but not one of the D-15 crates.
pub const XTASK: &str = "xtask";
/// The one crate allowed an HTTP client (R-REL-03).
pub const SYNTH: &str = "velme-synth";
/// HTTP client crates that may only appear under `SYNTH` (R-REL-03, INV-7).
pub const HTTP_CLIENTS: &[&str] = &["reqwest", "hyper", "ureq", "isahc", "surf", "attohttpc", "curl"];

/// A workspace member and what it declares it depends on.
pub struct Package {
    /// Cargo package name.
    pub name: String,
    /// Declared dependencies (normal, build and dev).
    pub deps: Vec<Dependency>,
}

/// One declared dependency edge.
pub struct Dependency {
    /// Cargo package name of the dependency.
    pub name: String,
    /// True for `[dev-dependencies]`.
    pub dev: bool,
}

/// Reads workspace members from `cargo metadata --no-deps --format-version 1` output.
pub fn packages_from_metadata(json: &str) -> Result<Vec<Package>> {
    let root: Value = serde_json::from_str(json).context("parsing cargo metadata")?;
    let packages = root
        .get("packages")
        .and_then(Value::as_array)
        .context("cargo metadata has no `packages`")?;
    packages
        .iter()
        .map(|p| {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .context("package without a name")?;
            let deps = p
                .get("dependencies")
                .and_then(Value::as_array)
                .context("package without `dependencies`")?
                .iter()
                .map(|d| {
                    let name = d
                        .get("name")
                        .and_then(Value::as_str)
                        .context("dependency without a name")?;
                    let dev = d.get("kind").and_then(Value::as_str) == Some("dev");
                    Ok(Dependency {
                        name: name.to_owned(),
                        dev,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Package {
                name: name.to_owned(),
                deps,
            })
        })
        .collect()
}

/// Returns one message per forbidden edge, sorted; empty means the graph is legal.
pub fn check(packages: &[Package]) -> Vec<String> {
    let is_workspace = |name: &str| name == XTASK || allowed_deps(name).is_some();
    let mut violations = Vec::new();
    for package in packages {
        let http = |dep: &Dependency| package.name != SYNTH && HTTP_CLIENTS.contains(&dep.name.as_str());
        violations.extend(package.deps.iter().filter(|d| http(d)).map(|d| {
            format!(
                "`{}` depends on HTTP client `{}`; only `{SYNTH}` may (R-REL-03)",
                package.name, d.name
            )
        }));
        if package.name == XTASK {
            continue;
        }
        let Some(may) = allowed_deps(&package.name) else {
            violations.push(format!("`{}` is not a D-15 crate (R-REL-01)", package.name));
            continue;
        };
        for dep in &package.deps {
            if !is_workspace(&dep.name) {
                continue;
            }
            let dev_test_support = dep.dev && dep.name == TEST_SUPPORT;
            if !may.contains(&dep.name.as_str()) && !dev_test_support {
                violations.push(format!(
                    "`{}` -> `{}` is not an allowed edge (INV-9, compiler/20 §2)",
                    package.name, dep.name
                ));
            }
        }
    }
    violations.sort();
    violations
}

/// `cargo metadata --no-deps --format-version 1` output for `manifest_path`.
pub fn metadata_json(manifest_path: &Path) -> Result<String> {
    let output = Command::new(crate::cargo_program())
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(manifest_path)
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    String::from_utf8(output.stdout).context("cargo metadata output is not UTF-8")
}

/// Runs `cargo metadata` on `manifest_path` and checks the result.
pub fn check_manifest(manifest_path: &Path) -> Result<Vec<String>> {
    Ok(check(&packages_from_metadata(&metadata_json(manifest_path)?)?))
}
