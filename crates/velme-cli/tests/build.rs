//! `velme build` on the scripted provider (`tooling/40` §2, `compiler/22` R-SYNTH-05): the whole flow from identity to
//! lock, through the binary. Needs the `test-provider` feature, which the gate turns on (D-94).
#![cfg(feature = "test-provider")]
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_ir::IR_VERSION;

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    examples:
        - Double(2) == 4

goal AddOne(n: Number) -> Number:
    plan: \"Add one.\"
    examples:
        - AddOne(1) == 2

goal Both(n: Number) -> Number:
    call:
        d = Double(n)
        result = AddOne(d)
";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-build").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    fs::write(dir.join("game.velme"), SOURCE).expect("source");
    dir
}

fn ir(goal: &str, op: &str, k: i64) -> String {
    let number = json!({"t": "Number"});
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal, "types": {},
           "inputs": [["n", number]], "output": number,
           "body": {"kind": "binary", "op": op, "left": {"kind": "input", "name": "n"},
                    "right": {"kind": "literal", "type": number, "value": k}}})
    .to_string()
}

fn double() -> String {
    ir("Double", "mul", 2)
}

fn add_one() -> String {
    ir("AddOne", "add", 1)
}

/// Writes a script of replies, as JSON strings (`tooling/40` §5.2).
fn script(dir: &Path, replies: &[String]) -> PathBuf {
    let path = dir.join("script.json");
    fs::write(&path, serde_json::to_string(replies).expect("json")).expect("script");
    path
}

fn velme(dir: &Path, args: &[&str], script: Option<&Path>) -> Out {
    let mut command = Command::new(env!("CARGO_BIN_EXE_velme"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_SCRIPT");
    if let Some(script) = script {
        command.env("VELME_SYNTH_SCRIPT", script);
    }
    let out = command.output().expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn artifacts(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(".velme/artifacts"))
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A build with a scripted provider prints the notice once, writes the lock and the store, and the goals then run; a
/// second, fully cached build prints no notice and makes no call (AC-SEC-09, AC-ART-01, R-SEC-12).
#[test]
fn ac_sec_09_the_notice_is_printed_once_and_a_cached_build_prints_none() {
    let dir = project("notice");
    let script = script(&dir, &[double(), add_one()]);
    let first = velme(&dir, &["build", "game.velme", "--provider", "scripted"], Some(&script));
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    assert_eq!(first.stderr.matches("Nothing is sent").count(), 1, "{}", first.stderr);
    assert!(first.stdout.contains("Double  ✓ built"), "{}", first.stdout);
    assert!(
        first.stdout.contains("Both  ✓ built by the compiler"),
        "{}",
        first.stdout
    );
    assert!(first.stdout.contains("2 provider calls"), "{}", first.stdout);
    assert!(dir.join("velme.lock").is_file());
    assert_eq!(artifacts(&dir).len(), 3);
    let run = velme(&dir, &["run", "game.velme", "--goal", "Both", "--arg", "n=4"], None);
    assert_eq!(
        (run.stdout.trim_end().ends_with('9'), run.code),
        (true, 0),
        "{}{}",
        run.stdout,
        run.stderr
    );
    let lock = fs::read(dir.join("velme.lock")).expect("lock");
    let before = artifacts(&dir);
    // Nothing is left to synthesize, so even the default provider, which has no key, is never asked for.
    let second = velme(&dir, &["build", "game.velme"], None);
    assert_eq!((second.code, second.stderr.as_str()), (0, ""), "{}", second.stdout);
    assert!(second.stdout.contains("0 provider calls"), "{}", second.stdout);
    assert_eq!(fs::read(dir.join("velme.lock")).expect("lock"), lock);
    assert_eq!(artifacts(&dir), before);
}

/// Without a key or a provider, a build that needs synthesis fails with `VL0405` (exit 2) and says how to set one up
/// (`tooling/40` R-CLI-12).
#[test]
fn a_build_that_needs_a_provider_it_has_not_got_is_vl0405() {
    let dir = project("unconfigured");
    let out = velme(&dir, &["build", "game.velme"], None);
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("[VL0405]"), "{}", out.stderr);
    assert!(!dir.join("velme.lock").exists());
    let unknown = velme(&dir, &["build", "game.velme", "--provider", "nope"], None);
    assert_eq!(unknown.code, 64, "{}", unknown.stderr);
}

/// A question is `VL0407` with exit 2 and the question quoted; another goal still builds (AC-SYNTH-22).
#[test]
fn ac_synth_22_a_question_exits_2_and_names_the_question() {
    let dir = project("question");
    let script = script(&dir, &[r#"{"question": "Round up?"}"#.to_owned(), add_one()]);
    let out = velme(&dir, &["build", "game.velme", "--provider", "scripted"], Some(&script));
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("[VL0407]") && out.stderr.contains("Round up?"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("[VL0409]"),
        "the wired goal is blocked: {}",
        out.stderr
    );
    assert!(out.stdout.contains("AddOne  ✓ built"), "{}", out.stdout);
    assert_eq!(artifacts(&dir).len(), 1);
}

/// `--json` carries the notice in `notices`, the goals' statuses and the summary (`tooling/40` §3.2, R-CLI-15).
#[test]
fn build_json_carries_notices_statuses_and_summary() {
    let dir = project("json");
    let script = script(&dir, &[r#"{"question": "Round up?"}"#.to_owned(), add_one()]);
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted", "--json"],
        Some(&script),
    );
    assert_eq!(out.code, 2);
    assert_eq!(out.stderr, "");
    let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(envelope["status"], "failed");
    assert_eq!(envelope["notices"].as_array().map(Vec::len), Some(1));
    let statuses: Vec<(String, String)> = envelope["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| {
            (
                r["goal"].as_str().unwrap_or("").to_owned(),
                r["status"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [("Double", "failed"), ("AddOne", "ok"), ("Both", "blocked")].map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    assert_eq!(envelope["summary"]["calls"], 2);
}

/// `check`, `explain` and `run` never construct a provider: with a script that doesn't exist named in the environment,
/// which building one would fail on, they behave as without it (AC-CMP-02, AC-CLI-07). `explain` needs no lock, no store
/// and no provider, and writes nothing.
#[test]
fn ac_cmp_02_commands_other_than_build_never_reach_a_provider() {
    let dir = project("no-provider");
    let missing = dir.join("missing.json");
    for args in [
        &["check", "game.velme"][..],
        &["explain", "game.velme", "--goal", "Both"][..],
    ] {
        let with = velme(&dir, args, Some(&missing));
        let without = velme(&dir, args, None);
        assert_eq!(
            (with.code, &with.stdout, &with.stderr),
            (without.code, &without.stdout, &without.stderr)
        );
        assert_eq!(with.code, 0, "{}", with.stderr);
    }
    assert!(!dir.join(".velme").exists() && !dir.join("velme.lock").exists());
}
