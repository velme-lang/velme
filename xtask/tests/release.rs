//! The release workflow (`delivery/52` §4, R-REL-13, AC-REL-04, D-143..D-146, D-149) and the security baseline (`tooling/41`
//! §6, D-147), checked as text: each job is the block under its two-space key in `jobs:`.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;

use xtask::workspace_root;

/// The guard on every real-release job: a push of a `v*` tag; a manual run is always a dry run (D-149).
const REAL_RELEASE: &str = "if: ${{ github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') }}";
/// Its negation, on the dry run's signing job.
const DRY_RUN: &str = "if: ${{ !(github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')) }}";

const TARGETS: [&str; 5] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

fn path(relative: &str) -> PathBuf {
    workspace_root().expect("root").join(relative)
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(path(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// The workflow: what precedes `jobs:`, and each job as `(id, block)` in file order. Comment lines are dropped.
fn workflow(relative: &str) -> (String, Vec<(String, String)>) {
    let text = read(relative);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    let jobs_at = lines.iter().position(|l| *l == "jobs:").expect("a jobs: key");
    let header = lines[..jobs_at].join("\n");
    let mut jobs: Vec<(String, String)> = Vec::new();
    for line in &lines[jobs_at + 1..] {
        let key = line.strip_prefix("  ").filter(|rest| !rest.starts_with(' '));
        match key.and_then(|rest| rest.strip_suffix(':')) {
            Some(id) => jobs.push((id.to_owned(), String::new())),
            None => {
                let (_, block) = jobs.last_mut().expect("a job before its steps");
                block.push_str(line);
                block.push('\n');
            }
        }
    }
    (header, jobs)
}

fn job<'a>(jobs: &'a [(String, String)], id: &str) -> &'a str {
    jobs.iter()
        .find(|(name, _)| name == id)
        .map(|(_, block)| block.as_str())
        .unwrap_or_else(|| panic!("release.yml has no `{id}` job"))
}

#[test]
fn ac_rel_04_only_a_tag_push_releases_and_a_manual_run_is_a_dry_run() {
    let (header, jobs) = workflow(".github/workflows/release.yml");
    assert!(header.contains("  push:\n    tags: [\"v*.*.*\"]"), "{header}");
    assert!(header.contains("  workflow_dispatch:"), "{header}");
    // D-149: no `dry_run` input; D-146: the dry run builds `0.1.0`.
    assert!(!header.contains("dry_run"), "{header}");
    assert!(
        header.contains("      version:") && header.contains("        default: \"0.1.0\""),
        "{header}"
    );
    // The version is a release version, the workspace's, and a tag must be on `main`; no input reaches a message.
    let verify = job(&jobs, "verify");
    for check in [
        "fetch-depth: 0",
        "re='^[0-9]+\\.[0-9]+\\.[0-9]+(-[0-9A-Za-z.-]+)?$'",
        "if ! [[ \"$version\" =~ $re ]]; then",
        "select(.name == \"velme-cli\") | .version",
        "if [ \"$version\" != \"$crates\" ]; then",
        "! git merge-base --is-ancestor \"$GITHUB_SHA\" origin/main",
    ] {
        assert!(verify.contains(check), "{check}");
    }
    for message in verify.lines().filter(|l| l.contains("::error::")) {
        assert!(!message.contains('$'), "{message}");
    }
}

#[test]
fn ac_rel_04_a_dry_run_verifies_builds_tests_compares_checksums_signs_and_publishes_nothing() {
    let (_, jobs) = workflow(".github/workflows/release.yml");
    assert!(job(&jobs, "verify").contains("run: cargo xtask verify"));

    let build = job(&jobs, "build");
    assert!(
        build.contains("        build: [1, 2]"),
        "each target is built twice (D-145)"
    );
    let tests = job(&jobs, "release-tests");
    for target in TARGETS {
        assert!(build.contains(&format!("          - {target}\n")), "build of {target}");
        assert!(
            build.contains(&format!("{{ target: {target}, os: ")),
            "runner for {target}"
        );
        assert!(
            tests.contains(&format!("{{ target: {target}, os: ")),
            "release tests on {target}"
        );
    }
    // D-149: glibc 2.35 is the floor.
    for runner in ["os: ubuntu-22.04 }", "os: ubuntu-22.04-arm }"] {
        assert!(build.contains(runner) && tests.contains(runner), "{runner}");
    }
    assert!(build.contains("cargo build --release --locked --target \"$TARGET\" -p velme-cli"));
    assert!(tests.contains("needs: [verify, build]"));
    // The builds run alongside `verify` (they take nothing from it); what ships waits for both.
    assert!(!build.contains("needs:") && !build.contains("needs.verify"));
    assert!(tests.contains("find examples -type f -name '*.velme'") && tests.contains("\"$BIN\" test --locked"));

    // Never vacuous: all five targets are compared.
    let reproducible = job(&jobs, "reproducible");
    assert!(reproducible.contains("needs: build") && reproducible.contains("if [ \"$a\" != \"$b\" ]"));
    assert!(reproducible.contains("shopt -s nullglob") && reproducible.contains("if [ \"$compared\" -ne 5 ]"));

    // D-149: deterministic archives with the licences, checksummed.
    let package = job(&jobs, "package");
    assert!(package.contains("needs: [verify, build, release-tests, reproducible]"));
    for step in [
        "tool: cargo-about@0.9.2",
        "-o THIRD-PARTY-LICENSES.txt .github/release/about.hbs",
        "LICENSE-MIT LICENSE-APACHE README.md THIRD-PARTY-LICENSES.txt",
        "tar --sort=name --mtime=\"@$epoch\" --owner=0 --group=0 --numeric-owner",
        "gzip -n -9 > \"dist/$name.tar.gz\"",
        "LC_ALL=C sort | zip -X -D -q -@ \"dist/$name.zip\"",
        "LC_ALL=C sha256sum velme-* | LC_ALL=C sort -k2 > SHA256SUMS",
        "actions/upload-artifact@",
    ] {
        assert!(package.contains(step), "{step}");
    }
    assert!(read(".github/release/about.toml").contains("\"CDLA-Permissive-2.0\""));

    // D-144: a throwaway key made in the job, never stored, and nothing sent to a transparency log.
    let sign = job(&jobs, "sign-dry-run");
    assert!(sign.contains(DRY_RUN), "{sign}");
    assert!(sign.contains("ssh-keygen -q -t ed25519 -N ''") && sign.contains("ssh-keygen -q -Y sign"));
    assert!(sign.contains("ssh-keygen -Y verify") && sign.contains("rm -f \"$key\""));
    assert!(sign.contains("actions/upload-artifact@") && !sign.contains("secrets."));

    let publish = job(&jobs, "publish-dry-run");
    assert!(!publish.contains("if:"), "the publish dry run always runs");
    assert!(publish.contains("run: cargo publish --workspace --dry-run --locked"));
}

#[test]
fn ac_rel_04_only_a_tag_push_releases_publishes_or_reads_a_secret() {
    let (_, jobs) = workflow(".github/workflows/release.yml");
    for id in ["attest", "github-release", "publish"] {
        assert!(job(&jobs, id).contains(REAL_RELEASE), "`{id}` is not guarded");
    }
    for (id, block) in &jobs {
        let guarded = block.contains(REAL_RELEASE);
        let releases = block.contains("gh release")
            || block.contains("attest-build-provenance")
            || block.contains("secrets.")
            || block
                .lines()
                .any(|l| l.contains("cargo publish") && !l.contains("--dry-run"));
        assert!(
            !releases || guarded,
            "`{id}` releases, publishes or reads a secret without the guard"
        );
    }
    // A guarded job's dependants are skipped with it, so nothing after it runs in a dry run either.
    let attest = job(&jobs, "attest");
    assert!(attest.contains("- id: attest") && attest.contains("${{ steps.attest.outputs.bundle-path }}"));
    let release = job(&jobs, "github-release");
    assert!(release.contains("needs: [verify, attest]") && release.contains("sha256sum -c SHA256SUMS"));
    let publish = job(&jobs, "publish");
    assert!(publish.contains("needs: [verify, github-release]") && publish.contains("environment: crates-io"));
    assert!(publish.contains("cargo publish --workspace --locked --no-verify \"${excluded[@]}\""));
}

#[test]
fn ac_rel_04_every_job_has_its_own_least_permissions_and_only_attest_gets_an_id_token() {
    let (header, jobs) = workflow(".github/workflows/release.yml");
    assert!(header.contains("permissions:\n  contents: read"), "{header}");
    for (id, block) in &jobs {
        let permissions: Vec<&str> = block
            .lines()
            .skip_while(|l| !l.starts_with("    permissions:"))
            .take_while(|l| l.starts_with("    permissions:") || l.starts_with("      "))
            .map(str::trim)
            .collect();
        let granted: &[&str] = match id.as_str() {
            "attest" => &["permissions:", "id-token: write", "attestations: write"],
            "github-release" => &["permissions:", "contents: write"],
            "reproducible" | "sign-dry-run" => &["permissions: {}"],
            _ => &["permissions:", "contents: read"],
        };
        assert_eq!(permissions, granted, "`{id}`");
        assert_eq!(block.contains("id-token"), id == "attest", "`{id}`");
    }
}

#[test]
fn ac_rel_04_builds_are_reproducible_on_fresh_runners() {
    let text = read(".github/workflows/release.yml");
    // No build cache in any release job (D-145).
    assert!(!text.contains("rust-cache") && !text.contains("actions/cache"));
    let (_, jobs) = workflow(".github/workflows/release.yml");
    let build = job(&jobs, "build");
    for setting in [
        // The second build is somewhere else, with another cargo home.
        "- { build: 2, dir: elsewhere/velme-second }",
        "path: ${{ matrix.dir }}",
        "cargo_home=\"$RUNNER_TEMP/cargo-home-second\"",
        "\"--remap-path-prefix=$workspace=velme\" \"--remap-path-prefix=$cargo_home=cargo\"",
        "CARGO_ENCODED_RUSTFLAGS=",
        "cflags=\"-ffile-prefix-map=$workspace=velme -ffile-prefix-map=$cargo_home=cargo\"",
        "-Clink-arg=-Brepro",
        "cflags=\"-Brepro\"",
        // ld64 hashes the debug map's object paths and mtimes into LC_UUID before the strip (R-REL-13).
        "flags+=(\"-Clink-arg=-Wl,-oso_prefix,$workspace/\")",
        "echo \"ZERO_AR_DATE=1\" >> \"$GITHUB_ENV\"",
        "echo \"SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)\"",
        "grep -qF -e \"$path\" -e \"${path//\\\\//}\"",
        "> dist/build-info.txt",
    ] {
        assert!(build.contains(setting), "{setting}");
    }
    // Git Bash rewrites a leading `/` (D-145): no flag uses the slash form.
    assert!(!build.contains("/Brepro") && !build.contains("=/velme"));
    // ThinLTO between codegen units named symbols by a hash that differed between the two directories (R-REL-13).
    assert!(read("Cargo.toml").contains("[profile.release]\ncodegen-units = 1\n"));
}

/// Every action in the release and CodeQL workflows is pinned by a full commit SHA, as in `ci.yml`.
#[test]
fn ac_rel_04_release_actions_are_pinned_by_commit() {
    for file in [".github/workflows/release.yml", ".github/workflows/codeql.yml"] {
        let uses = read(file);
        let uses: Vec<&str> = uses
            .lines()
            .map(|l| l.trim().trim_start_matches("- "))
            .filter_map(|l| l.strip_prefix("uses: "))
            .collect();
        assert!(!uses.is_empty(), "{file}");
        for action in uses {
            let sha = action
                .split_once('@')
                .map(|(_, rest)| rest.split(' ').next().unwrap_or(""));
            assert!(
                sha.is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())),
                "{file}: {action}"
            );
        }
    }
}

#[test]
fn d_147_the_security_baseline_is_in_place() {
    assert!(read("SECURITY.md").contains("## Reporting a vulnerability"));
    let owners = read(".github/CODEOWNERS");
    // The paths of `delivery/52` R-REL-05.
    for owned in [
        "/docs/spec/",
        "/crates/velme-ir/",
        "/crates/velme-sema/",
        "/crates/velme-runtime/",
        "/crates/velme-wasm/",
        "/crates/velme-synth/",
        "/xtask/",
        "/.github/workflows/",
        "/deny.toml",
        "/rust-toolchain.toml",
        "/Cargo.lock",
    ] {
        assert!(
            owners.lines().any(|l| l.split_whitespace().next() == Some(owned)),
            "{owned}"
        );
    }
    let dependabot = read(".github/dependabot.yml");
    for ecosystem in ["cargo", "github-actions"] {
        assert!(
            dependabot.contains(&format!("package-ecosystem: {ecosystem}\n")),
            "{ecosystem}"
        );
    }
    assert_eq!(
        dependabot
            .matches(
                "cooldown:
      default-days: 7"
            )
            .count(),
        2
    );
    assert_eq!(dependabot.matches("groups:").count(), 2);
    let (header, jobs) = workflow(".github/workflows/codeql.yml");
    assert!(header.contains("permissions:\n  contents: read"), "{header}");
    let analyze = job(&jobs, "analyze");
    assert!(analyze.contains("languages: actions"));
    assert!(analyze.contains("    permissions:\n      contents: read\n      security-events: write\n"));
}
