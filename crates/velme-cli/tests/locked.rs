//! `--locked`, `--offline` and `velme artifact` through the binary (`tooling/40` R-CLI-04, R-CLI-05, R-CLI-22, D-106,
//! D-107) on the committed fixture projects and copies of them. None of these tests has a provider to reach.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use velme_runtime::{Lock, Store};
use velme_test_support::{install, program, read, repo};

const ADD: &str = "add.velme";

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Runs `velme` in `dir` with every provider setting of the user's environment removed.
fn velme(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_velme"))
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_SCRIPT")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_URL")
        .output()
        .expect("velme runs");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is UTF-8"),
        code: out.status.code().expect("an exit code"),
    }
}

/// Every file under `dir` with its bytes, by path from `dir`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .expect("inside")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(name, fs::read(&path).expect("file"));
            }
        }
    }
    files
}

/// A copy of the committed fixture project `name`, in a fresh directory for `test`.
fn copy(name: &str, test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("locked").join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    for (file, bytes) in tree(&repo(&format!("tests/fixtures/run/{name}"))) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        fs::write(path, bytes).expect("copied");
    }
    dir
}

/// The copy of `add` with its plan edited: its lock entry is stale (its artifact is untouched).
fn stale(test: &str) -> PathBuf {
    let dir = copy("add", test);
    let source = read(&dir.join(ADD)).replace("Add a and b.", "Add both numbers.");
    fs::write(dir.join(ADD), source).expect("edited");
    dir
}

/// The copy of `add` whose artifact is replaced by one that is right except at `a == 2`, with a lock entry to match: a
/// hash-consistent artifact that fails the example `Add(2, 3) == 5` and nothing else the run below asks (D-46).
fn broken(test: &str) -> PathBuf {
    broken_at(test, 2)
}

/// [`broken`] for the input `a == at`: with `at` outside the examples, only the generated inputs can find it.
fn broken_at(test: &str, at: i64) -> PathBuf {
    let number = json!({"t": "Number"});
    let input = |name: &str| json!({"kind": "input", "name": name});
    let sum = json!({"kind": "binary", "op": "add", "left": input("a"), "right": input("b")});
    let plus_one =
        json!({"kind": "binary", "op": "add", "left": sum, "right": {"kind": "literal", "type": number, "value": 1}});
    broken_with(test, at, plus_one)
}

/// [`broken_at`] with `then` as what the goal does for `a == at`.
fn broken_with(test: &str, at: i64, then: Value) -> PathBuf {
    let dir = copy("add", test);
    fs::remove_dir_all(dir.join(".velme")).expect("store removed");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let source = read(&dir.join(ADD));
    let number = json!({"t": "Number"});
    let input = |name: &str| json!({"kind": "input", "name": name});
    let sum = json!({"kind": "binary", "op": "add", "left": input("a"), "right": input("b")});
    let ir = json!({
        "ir_version": "0.1", "builtins_version": "0.1", "goal": "Add", "types": {},
        "inputs": [["a", number], ["b", number]], "output": number,
        "body": {"kind": "if",
                 "cond": {"kind": "binary", "op": "eq", "left": input("a"),
                          "right": {"kind": "literal", "type": number, "value": at}},
                 "then": then,
                 "else": sum}
    });
    install(&dir, ADD, &program(&source), &ir.to_string());
    fs::remove_dir_all(dir.join(".velme/tmp")).expect("temporary directory");
    dir
}

fn json_of(run: &Run) -> Value {
    serde_json::from_str(&run.stdout).expect("--json prints one JSON document")
}

fn codes(envelope: &Value) -> Vec<String> {
    envelope["results"]
        .as_array()
        .expect("results")
        .iter()
        .flat_map(|r| r["diagnostics"].as_array().expect("diagnostics").iter())
        .chain(envelope["diagnostics"].as_array().expect("diagnostics").iter())
        .map(|d| d["code"].as_str().expect("a code").to_owned())
        .collect()
}

// ---- --locked ----

/// `velme run --locked` with a current lock runs, offline too, and changes nothing (AC-CLI-02).
#[test]
fn ac_cli_02_run_locked_with_a_current_lock_needs_no_provider() {
    let dir = copy("add", "current");
    let before = tree(&dir);
    for flags in [&["--locked"][..], &["--locked", "--offline"], &["--offline"]] {
        let mut args = vec!["run", ADD, "--goal", "Add", "--arg", "a=2", "--arg", "b=3"];
        args.extend(flags);
        let run = velme(&dir, &args);
        assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{flags:?}");
        assert!(run.stdout.trim_end().ends_with('5'), "{}", run.stdout);
    }
    assert_eq!(tree(&dir), before);
}

/// `velme run --locked` with a stale entry exits 4 with `VL0702` naming the goal, and leaves the lock alone (AC-CLI-03).
#[test]
fn ac_cli_03_run_locked_with_a_stale_entry_exits_4_and_leaves_the_lock_unchanged() {
    let dir = stale("run_stale");
    let before = tree(&dir);
    let run = velme(
        &dir,
        &["run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2", "--locked"],
    );
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("`Add` changed since it was last built — run `velme build`.  [VL0702]"),
        "{}",
        run.stderr
    );
    assert_eq!(tree(&dir), before);
}

/// Input and lock problems are reported together, and the usage error decides the exit code (AC-CLI-13, R-CLI-16).
#[test]
fn ac_cli_13_run_reports_input_and_lock_problems_together() {
    let dir = stale("both_problems");
    let bad = dir.join("input.json");
    fs::write(&bad, r#"{"a": "one", "b": 2}"#).expect("input");
    let run = velme(
        &dir,
        &["run", ADD, "--goal", "Add", "--input", "input.json", "--locked"],
    );
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0902]"), "{}", run.stderr);
    assert!(run.stderr.contains("[VL0702]"), "{}", run.stderr);
    let run = velme(
        &dir,
        &[
            "run",
            ADD,
            "--goal",
            "Add",
            "--input",
            "input.json",
            "--locked",
            "--json",
        ],
    );
    assert_eq!(run.code, 64);
    let envelope = json_of(&run);
    let mut found = codes(&envelope);
    found.sort();
    assert_eq!(found, ["VL0702", "VL0902"]);
}

/// `--locked` on `build` re-verifies every lock hit with no provider: a hash-consistent artifact that fails an example
/// is `VL0702` with that cause, and the lock and the store are unchanged (AC-CLI-18, AC-ART-11).
#[test]
fn ac_cli_18_build_locked_reverifies_and_writes_nothing() {
    let dir = broken("build_locked");
    let before = tree(&dir);
    let run = velme(&dir, &["build", ADD, "--locked"]);
    assert_eq!(run.code, 4, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stderr
            .contains("`Add` no longer passes its examples — run `velme build`.  [VL0702]"),
        "{}",
        run.stderr
    );
    assert!(run.stdout.contains("Add  ✗"), "{}", run.stdout);
    assert_eq!(tree(&dir), before);
    // A source that no longer declares a goal leaves the lock entry it had alone, and is no error.
    let fine = copy("add", "build_locked_ok");
    let mut lock = Lock::read(&fine).expect("lock").expect("a lock");
    let entry = lock.entry(ADD, "Add").expect("an entry").clone();
    lock.insert(velme_runtime::Entry {
        name: "Gone".to_owned(),
        ..entry
    });
    lock.write(&fine).expect("lock written");
    let before = tree(&fine);
    let run = velme(&fine, &["build", ADD, "--locked"]);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);
    assert!(run.stdout.contains("Add  ✓ up to date"), "{}", run.stdout);
    assert_eq!(tree(&fine), before);
}

/// `velme test --locked` fails on an example the artifact breaks, though the file is hash-consistent; `velme run` with
/// the same artifact doesn't notice, having no example to run (AC-ART-11, D-46). One broken only for an input no example
/// names is found by the generated inputs the same `velme test --locked` runs after the examples.
#[test]
fn ac_art_11_test_locked_finds_the_broken_example_and_run_does_not() {
    let dir = broken("art_11");
    let test = velme(&dir, &["test", ADD, "--locked"]);
    assert_eq!(test.code, 3, "{}{}", test.stdout, test.stderr);
    assert!(test.stderr.contains("[VL0502]"), "{}", test.stderr);
    assert!(test.stderr.contains("Add(2, 3)"), "{}", test.stderr);
    let run = velme(
        &dir,
        &["run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2", "--locked"],
    );
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);
    assert!(run.stdout.trim_end().ends_with('3'), "{}", run.stdout);

    let dir = broken_at("art_11_generated", 1_000_000);
    let test = velme(&dir, &["test", ADD, "--locked"]);
    assert_eq!(test.code, 3, "{}{}", test.stdout, test.stderr);
    assert!(
        test.stderr.contains("[VL0501]") && !test.stderr.contains("[VL0502]"),
        "{}",
        test.stderr
    );
    let run = velme(
        &dir,
        &[
            "run",
            ADD,
            "--goal",
            "Add",
            "--arg",
            "a=1000000",
            "--arg",
            "b=2",
            "--locked",
        ],
    );
    assert_eq!(
        run.code, 3,
        "the run's own checks see it, but only for this input: {}",
        run.stderr
    );
}

/// A locked goal that fails at run time on a generated input is reported by `velme test` as that failure, with its own code
/// and exit, not as `VL0503`, which is a build's (D-107).
#[test]
fn a_runtime_failure_on_a_generated_input_keeps_its_own_code_in_test() {
    let number = json!({"t": "Number"});
    let input = |name: &str| json!({"kind": "input", "name": name});
    let sum = json!({"kind": "binary", "op": "add", "left": input("a"), "right": input("b")});
    let zero = json!({"kind": "literal", "type": number, "value": 0});
    let divide = json!({"kind": "binary", "op": "div", "left": sum, "right": zero});
    let dir = broken_with("test_runtime_failure", 1_000_000, divide);
    let test = velme(&dir, &["test", ADD, "--locked", "--json"]);
    assert_eq!(test.code, 3, "{}{}", test.stdout, test.stderr);
    let envelope = json_of(&test);
    let found = codes(&envelope);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        !["VL0503", "VL0501", "VL0502"].contains(&found[0].as_str()),
        "{found:?}"
    );
    assert!(
        envelope["results"][0]["diagnostics"][0]["notes"]
            .to_string()
            .contains("input:"),
        "{envelope}"
    );
}

/// `--build` with `--locked` is a usage error, `VL0902` and exit 64, and nothing is built or run (AC-CLI-11, R-CLI-14).
#[test]
fn ac_cli_11_build_with_locked_is_a_usage_error() {
    let dir = stale("build_locked_flags");
    let before = tree(&dir);
    let run = velme(
        &dir,
        &[
            "run", ADD, "--goal", "Add", "--build", "--locked", "--arg", "a=1", "--arg", "b=2",
        ],
    );
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("[VL0902]"), "{}", run.stderr);
    let run = velme(&dir, &["test", ADD, "--locked", "--build", "--json"]);
    assert_eq!(run.code, 64);
    let envelope = json_of(&run);
    assert_eq!(envelope["status"], "failed");
    assert_eq!(envelope["diagnostics"][0]["code"], "VL0902");
    assert_eq!(tree(&dir), before);
}

// ---- --offline ----

/// `--offline` with a goal that isn't fresh in the lock is `VL0404` in the offline wording for every provider, with no
/// contact and no notice, and adding `--locked` makes it `VL0702`. None of the providers is set up here, so one that was
/// constructed would say `VL0405` or `VL0902` instead (AC-CLI-19, AC-SYNTH-08, R-CLI-05).
#[test]
fn ac_cli_19_offline_build_with_a_stale_goal_is_vl0404_for_every_provider() {
    let dir = stale("offline");
    let before = tree(&dir);
    // A service that would see any contact, whichever way a provider is built: nothing may reach it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("non-blocking");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    for flag in ["--offline", "--locked"] {
        let run = velme(
            &dir,
            &["build", ADD, flag, "--provider", "external", "--external-url", &url],
        );
        assert_eq!(
            run.code,
            if flag == "--locked" { 4 } else { 2 },
            "{flag}: {}",
            run.stderr
        );
        assert_eq!(
            listener.accept().map(|_| ()).map_err(|e| e.kind()),
            Err(std::io::ErrorKind::WouldBlock),
            "{flag} contacted the service"
        );
    }
    let providers = [
        vec!["--provider", "anthropic"],
        vec!["--provider", "ollama", "--model", "llama3"],
        vec!["--provider", "external", "--external-url", "http://127.0.0.1:9"],
        vec!["--provider", "replay"],
        vec!["--provider", "scripted"],
        vec![],
    ];
    for provider in providers {
        let mut args = vec!["build", ADD, "--offline"];
        args.extend(&provider);
        let run = velme(&dir, &args);
        assert_eq!(run.code, 2, "{provider:?}: {}{}", run.stdout, run.stderr);
        assert!(
            run.stderr
                .contains("`Add` needs building, and `--offline` is on.  [VL0404]"),
            "{provider:?}: {}",
            run.stderr
        );
        assert!(!run.stderr.contains("Sending"), "{provider:?}: {}", run.stderr);
        assert!(run.stdout.contains("0 provider calls"), "{}", run.stdout);
        args.push("--locked");
        let run = velme(&dir, &args);
        assert_eq!(run.code, 4, "{provider:?}: {}{}", run.stdout, run.stderr);
        assert!(run.stderr.contains("[VL0702]"), "{provider:?}: {}", run.stderr);
        assert!(!run.stderr.contains("VL0404"), "{provider:?}: {}", run.stderr);
    }
    assert_eq!(tree(&dir), before);
    // A current lock builds offline, and that is not a change.
    let fresh = copy("add", "offline_fresh");
    let before = tree(&fresh);
    let run = velme(&fresh, &["build", ADD, "--offline", "--provider", "scripted"]);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);
    assert!(run.stdout.contains("Add  ✓ up to date"), "{}", run.stdout);
    assert_eq!(tree(&fresh), before);
}

// ---- velme artifact ----

/// `velme artifact` prints the hash, the manifest lines and the pretty IR; `--json` gives `{artifact, manifest, ir}`
/// (AC-CLI-20, R-CLI-22).
#[test]
fn ac_cli_20_artifact_shows_hash_manifest_and_ir() {
    let dir = copy("add", "artifact");
    let before = tree(&dir);
    let run = velme(&dir, &["artifact", ADD, "--goal", "Add"]);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""));
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let hash = lock.entry(ADD, "Add").expect("an entry").artifact.to_string();
    assert_eq!(run.stdout.lines().next(), Some(hash.as_str()));
    let (head, ir) = run.stdout.split_once("\n\n").expect("a blank line before the IR");
    insta::assert_snapshot!("artifact_add_text", head);
    let ir: Value = serde_json::from_str(ir).expect("the IR is JSON");
    assert_eq!(ir["goal"], "Add");
    assert!(ir["body"].is_object());
    assert!(run.stdout.contains("\n  \"ir_version\""), "{}", run.stdout);

    let run = velme(&dir, &["artifact", ADD, "--goal", "Add", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let envelope = json_of(&run);
    // The artifact sits beside `result`, which a goal's value alone fills (D-111).
    velme_test_support::schema::assert_cli_envelope(&envelope);
    assert!(envelope["results"][0].get("result").is_none(), "{envelope}");
    let result = &envelope["results"][0]["artifact"];
    assert_eq!(result["artifact"], hash.as_str());
    assert_eq!(result["ir"], ir);
    // Every manifest field is on a line of its own in the text.
    let manifest = result["manifest"].as_object().expect("a manifest");
    for key in manifest.keys() {
        assert!(
            head.lines().any(|l| l.starts_with(&format!("{key}: "))),
            "{key}: {head}"
        );
    }
    assert_eq!(head.lines().count(), manifest.len() + 1);
    assert_eq!(tree(&dir), before);
}

/// A stale entry, a missing file and a damaged file fail as they do for `velme run`: `VL0702`, `VL0701` and `VL0703`,
/// and no artifact is shown (AC-CLI-20, R-CLI-22).
#[test]
fn ac_cli_20_artifact_fails_with_the_codes_of_run() {
    let cases = [("stale", "VL0702"), ("missing", "VL0701"), ("damaged", "VL0703")];
    for (case, code) in cases {
        let dir = if case == "stale" {
            stale("artifact_stale")
        } else {
            copy("add", &format!("artifact_{case}"))
        };
        let lock = Lock::read(&dir).expect("lock").expect("a lock");
        let file = Store::new(&dir).path(lock.entry(ADD, "Add").expect("an entry").artifact);
        match case {
            "missing" => fs::remove_file(&file).expect("removed"),
            "damaged" => {
                let mut bytes = fs::read(&file).expect("artifact");
                bytes.push(b' ');
                fs::write(&file, bytes).expect("damaged");
            }
            _ => {}
        }
        let run = velme(&dir, &["artifact", ADD, "--goal", "Add"]);
        assert_eq!((run.code, run.stdout.as_str()), (4, ""), "{case}: {}", run.stderr);
        assert!(run.stderr.contains(&format!("[{code}]")), "{case}: {}", run.stderr);
        let run = velme(&dir, &["run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2"]);
        assert!(run.stderr.contains(&format!("[{code}]")), "{case}: {}", run.stderr);
    }
    let dir = copy("add", "artifact_unknown");
    let run = velme(&dir, &["artifact", ADD, "--goal", "Nope"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
}

// ---- AC-ART-07 ----

/// The trace of a built fixture project, run locked and offline from another directory, is the golden trace with its
/// durations left out. The fixture is committed and the test runs on every OS of the CI matrix, which is how a clone on
/// another OS gives the same result and trace (AC-ART-07, AC-RDM-09, D-107).
#[test]
fn ac_art_07_a_committed_built_project_runs_locked_and_offline_to_the_golden_trace() {
    let file = repo("tests/fixtures/run/level_summary/level_summary.velme");
    let file = file.to_str().expect("UTF-8 path");
    let elsewhere = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let level = r#"level={"enemy_count":7,"treasure_count":3,"base_score":120}"#;
    let args = [
        "trace",
        file,
        "--goal",
        "CreateLevelSummary",
        "--arg",
        level,
        "--jobs",
        "4",
        "--json",
    ];
    let plain = velme(elsewhere, &args);
    let mut offline = args.to_vec();
    offline.extend(["--locked", "--offline"]);
    let run = velme(elsewhere, &offline);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);
    // The timing lines are the only ones that differ from run to run.
    let timing = ["\"start_us\"", "\"end_us\"", "\"duration_us\""];
    let printed = |run: &Run| -> String {
        run.stdout
            .lines()
            .filter(|line| !timing.iter().any(|key| line.trim_start().starts_with(key)))
            .map(|line| format!("{line}\n"))
            .collect()
    };
    assert_eq!(printed(&run), printed(&plain));
    insta::assert_snapshot!("ac_art_07_golden_trace", printed(&run));
    // The fixture is a whole project: every goal of the file has a lock entry.
    let goals = program(&read(&repo("tests/fixtures/run/level_summary/level_summary.velme")))
        .goals
        .len();
    let lock = Lock::read(&repo("tests/fixtures/run/level_summary"))
        .expect("lock")
        .expect("a lock");
    assert_eq!(lock.entries().len(), goals);
}

/// `--offline` and `--locked` construct no provider but still check the provider name and the flag values, with the
/// `VL0902` a build gives, and never read a key (R-CLI-21).
#[test]
fn offline_and_locked_still_reject_a_bad_provider_name_or_url() {
    let dir = copy("add", "bad_flags");
    for flag in ["--offline", "--locked"] {
        let run = velme(&dir, &["build", ADD, flag, "--provider", "bogus"]);
        assert_eq!(run.code, 64, "{flag}: {}", run.stderr);
        assert!(
            run.stderr.contains("I don't know a provider called `bogus`.  [VL0902]"),
            "{}",
            run.stderr
        );
        let run = velme(&dir, &["build", ADD, flag, "--external-url", "ftp://x", "--json"]);
        assert_eq!(run.code, 64, "{flag}: {}", run.stderr);
        assert_eq!(json_of(&run)["diagnostics"][0]["code"], "VL0902");
    }
}

/// Under `--offline` a lock hit that fails re-verification keeps its entry: nothing was re-synthesized, so nothing is
/// dropped or written (D-100, D-106).
#[test]
fn an_offline_build_keeps_the_entry_of_a_lock_hit_that_fails_verification() {
    let dir = broken("offline_broken");
    let before = tree(&dir);
    let run = velme(&dir, &["build", ADD, "--offline"]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("[VL0404]"), "{}", run.stderr);
    assert_eq!(tree(&dir), before);
}

/// The IR of `velme artifact` stays JSON when a text literal holds characters R-CLI-17 makes visible (R-CLI-22).
#[test]
fn artifact_text_keeps_the_ir_parseable_with_unsafe_characters() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("locked")
        .join("unsafe_literal");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    let source = "language: velme/0.1\n\ngoal Greet(n: Number) -> Text:\n    plan: \"Greet.\"\n";
    fs::write(dir.join("greet.velme"), source).expect("source");
    let text = "a\u{7f}b\u{202e}c\u{2028}d";
    let ir = json!({
        "ir_version": "0.1", "builtins_version": "0.1", "goal": "Greet", "types": {},
        "inputs": [["n", {"t": "Number"}]], "output": {"t": "Text"},
        "body": {"kind": "literal", "type": {"t": "Text"}, "value": text}
    });
    install(&dir, "greet.velme", &program(source), &ir.to_string());
    let run = velme(&dir, &["artifact", "greet.velme", "--goal", "Greet"]);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""));
    assert!(
        !run.stdout.contains(['\u{7f}', '\u{202e}', '\u{2028}']),
        "{}",
        run.stdout
    );
    let (_, ir) = run.stdout.split_once("\n\n").expect("a blank line before the IR");
    let ir: Value = serde_json::from_str(ir).expect("the IR is still JSON");
    assert_eq!(ir["body"]["value"], text);
}
