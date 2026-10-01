//! `velme check` as a learner runs it: output, `--json` and exit codes (`tooling/40` §3–4, `reference/90` AC-ERR-03).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use velme_test_support::velme_command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Runs `velme` from the repository root, so paths in its output are the relative ones given here.
fn velme(args: &[&str]) -> Run {
    let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
        .args(args)
        .current_dir(repo_root())
        .env_remove("NO_COLOR")
        .output()
        .expect("velme runs");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is UTF-8"),
        code: out.status.code().expect("velme exits with a code"),
    }
}

fn json(run: &Run) -> Value {
    serde_json::from_str(&run.stdout).expect("--json prints one JSON document")
}

/// The syntax part of AC-CLI-01; the other four ✓ lines arrive with M2 and later.
#[test]
fn ac_cli_01_parse_part_valid_file_prints_parsed() {
    let run = velme(&["check", "examples/beginner/hello.velme"]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        ("✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ Checks valid\n", "", 0)
    );
}

/// `CalculateScore("hello")` is rejected by `velme check`, before anything runs.
#[test]
fn ac_rdm_04_type_mismatch_rejects_the_call_before_execution() {
    let run = velme(&["check", "tests/golden/sema/reject/call_block.velme"]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("Error: Expected Player, but got Text.  [VL0204]"),
        "{}",
        run.stderr
    );
}

#[test]
fn ac_rdm_05_cycle_fails_compilation_naming_it() {
    let run = velme(&["check", "tests/golden/sema/reject/call_cycle.velme"]);
    assert_eq!((run.stdout.as_str(), run.code), ("✓ Parsed\n✓ Types valid\n", 1));
    assert!(
        run.stderr
            .contains("Error: These goals call each other in a circle: B → C → B.  [VL0304]"),
        "{}",
        run.stderr
    );
}

/// Until the examples commit their locks and the examples-as-tests step runs `velme test --locked` on each
/// (`delivery/51` §2, D-141), every example at least parses without a warning.
#[test]
fn every_example_parses_cleanly() {
    let mut checked = 0;
    for dir in std::fs::read_dir(repo_root().join("examples")).expect("examples dir") {
        let dir = dir.expect("entry").path();
        if !dir.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&dir).expect("example folder") {
            let file = file.expect("entry").path();
            if file.extension().is_none_or(|e| e != "velme") {
                continue;
            }
            let rel = file
                .strip_prefix(repo_root())
                .expect("under the repo")
                .to_str()
                .expect("UTF-8 path");
            let run = velme(&["check", rel]);
            assert_eq!(
                (run.stdout.as_str(), run.stderr.as_str(), run.code),
                ("✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ Checks valid\n", "", 0),
                "{rel}"
            );
            checked += 1;
        }
    }
    assert!(checked > 1, "found {checked} examples");
}

#[test]
fn ac_cli_01_parse_part_syntax_error_exits_1_with_a_caret() {
    let run = velme(&["check", "tests/golden/parser/reject/tab_then_errors.velme"]);
    assert_eq!((run.stdout.as_str(), run.code), ("", 1));
    insta::assert_snapshot!(run.stderr);
}

#[test]
fn warnings_are_shown_but_do_not_fail() {
    let run = velme(&["check", "tests/golden/parser/reject/lint_naming.velme"]);
    assert_eq!(
        (run.stdout.as_str(), run.code),
        ("✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ Checks valid\n", 0)
    );
    assert!(run.stderr.starts_with("Warning: "), "{}", run.stderr);
    assert_eq!(
        json(&velme(&[
            "check",
            "--json",
            "tests/golden/parser/reject/lint_naming.velme"
        ]))["status"],
        "ok"
    );
}

/// AC-ERR-03 over the whole reject corpus: every human headline ends with its code, in the same order as `--json`.
#[test]
fn ac_err_03_every_rendered_diagnostic_ends_with_its_code() {
    for rel in reject_files() {
        let human = velme(&["check", &rel]);
        let headlines: Vec<&str> = human
            .stderr
            .lines()
            .filter(|l| l.starts_with("Error: ") || l.starts_with("Warning: "))
            .collect();
        let json = json(&velme(&["check", &rel, "--json"]));
        let codes: Vec<String> = json["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|d| d["code"].as_str().expect("code").to_owned())
            .collect();
        assert!(!headlines.is_empty(), "{rel}");
        assert_eq!(headlines.len(), codes.len().min(20), "{rel}");
        for (line, code) in headlines.iter().zip(&codes) {
            assert!(line.ends_with(&format!("  [{code}]")), "{rel}: {line}");
        }
    }
}

/// Every golden file with a diagnostic, relative to the repository root, sorted.
fn reject_files() -> Vec<String> {
    let mut files = Vec::new();
    for dir in REJECT_DIRS {
        for entry in std::fs::read_dir(repo_root().join(dir)).expect("reject dir") {
            let name = entry.expect("entry").file_name().into_string().expect("UTF-8 name");
            if name.ends_with(".velme") {
                files.push(format!("{dir}/{name}"));
            }
        }
    }
    files.sort();
    files
}

/// The golden folders of files with a diagnostic, per phase.
const REJECT_DIRS: [&str; 2] = ["tests/golden/parser/reject", "tests/golden/sema/reject"];

/// AC-CMP-04: `--json` for every golden error file matches its snapshot.
#[test]
fn ac_cmp_04_json_for_every_golden_error_file() {
    let snapshot = |dir: &str, path: &Path| {
        let name = path.file_name().and_then(|n| n.to_str()).expect("UTF-8 file name");
        let rel = format!("{dir}/{name}");
        let run = velme(&["check", "--json", &rel]);
        let json = json(&run);
        let expected = if json["status"] == "ok" { 0 } else { 1 };
        assert_eq!(run.code, expected, "{rel}");
        // Rendered by serde_json, not insta's serializer: with `arbitrary_precision` (unified in from `velme-ir`)
        // insta would print each number as serde_json's private token map.
        insta::assert_snapshot!(serde_json::to_string_pretty(&json).expect("JSON value serializes"));
    };
    insta::glob!("../../../tests/golden/parser/reject", "*.velme", |path| {
        snapshot("tests/golden/parser/reject", path);
    });
    insta::glob!("../../../tests/golden/sema/reject", "*.velme", |path| {
        snapshot("tests/golden/sema/reject", path);
    });
}

#[test]
fn json_and_human_messages_are_the_same_text() {
    let rel = "tests/golden/parser/reject/three_errors.velme";
    let human = velme(&["check", rel]).stderr;
    for d in json(&velme(&["check", "--json", rel]))["diagnostics"]
        .as_array()
        .expect("array")
    {
        let message = d["message"].as_str().expect("message");
        assert!(human.contains(&format!(": {message}  [")), "{message}");
    }
}

#[test]
fn missing_file_is_vl0901_and_exits_64() {
    let run = velme(&["check", "no/such/file.velme"]);
    assert_eq!((run.stdout.as_str(), run.code), ("", 64));
    assert!(
        run.stderr
            .starts_with("Error: I couldn't find `no/such/file.velme`.  [VL0901]\n"),
        "{}",
        run.stderr
    );
    // `--json` is global, so it may come first (`tooling/40` §2.1).
    let json = json(&velme(&["--json", "check", "no/such/file.velme"]));
    assert_eq!(json["diagnostics"][0]["code"], "VL0901");
    assert_eq!(json["diagnostics"][0]["span"]["line"], 1);
}

#[test]
fn invalid_utf8_is_vl0901_and_points_at_the_byte() {
    let dir = std::env::temp_dir().join(format!("velme-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("latin1.velme");
    std::fs::write(&file, b"goal A() -> Text:\n    plan: \"caf\xe9\"\n").expect("write");
    let run = velme(&["check", "--json", file.to_str().expect("UTF-8 path")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(run.code, 64);
    let span = &json(&run)["diagnostics"][0]["span"];
    assert_eq!((span["line"].as_u64(), span["column"].as_u64()), (Some(2), Some(15)));
}

/// R-CLI-17: `--json` escapes control and bidi characters too, as `\uXXXX`.
#[test]
fn json_escapes_bidi_characters() {
    let dir = std::env::temp_dir().join(format!("velme-cli-bidi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    // The file name carries the characters into a JSON string.
    let file = dir.join("bi\u{202e}di\u{85}.velme");
    std::fs::write(&file, "# evil \u{202e} comment\ntype A:\n    x: Number\n").expect("write");
    let path = file.to_str().expect("UTF-8 path");
    let run = velme(&["check", "--json", path]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(run.code, 0);
    assert!(!run.stdout.contains(['\u{202e}', '\u{85}']), "{}", run.stdout);
    assert!(run.stdout.contains("bi\\u202edi\\u0085.velme"), "{}", run.stdout);
    let diag = &json(&run)["diagnostics"][0];
    // Shown from the project root, here the file's own directory (R-CLI-19, D-82).
    assert_eq!(diag["file"], "bi\u{202e}di\u{85}.velme");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("comment") && m.contains("U+202E"))
    );
}

#[test]
fn usage_errors_exit_64() {
    for args in [
        &[][..],
        &["check"],
        &["check", "a.velme", "b.velme"],
        &["check", "--nope"],
        &["frobnicate"],
    ] {
        let run = velme(args);
        assert_eq!(run.code, 64, "{args:?}");
        assert!(run.stderr.starts_with("usage: velme check FILE"), "{args:?}");
    }
}

/// A call tree that needs 129 invocations is rejected by `velme check` with `VL0605` before anything runs; 128 is the
/// limit (AC-RUN-06, `runtime/30` §7).
#[test]
fn ac_run_06_a_call_tree_of_129_invocations_is_rejected_before_execution() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check_ac_run_06");
    std::fs::create_dir_all(&dir).expect("directory");
    let tree = |calls: usize| {
        let lines: String = (0..calls).map(|i| format!("        c{i} = Leaf(x)\n")).collect();
        format!(
            "language: velme/0.1\n\ngoal Leaf(x: Number) -> Number:\n    plan: \"Return x.\"\n\n\
             goal Wide(x: Number) -> Number:\n    call:\n{lines}    plan: \"Add them.\"\n"
        )
    };
    let check = |name: &str, calls: usize| {
        let file = dir.join(name);
        std::fs::write(&file, tree(calls)).expect("source");
        velme(&["check", file.to_str().expect("UTF-8 path")])
    };
    // `Wide` itself and 127 calls are 128 invocations.
    let ok = check("ok.velme", 127);
    assert_eq!(
        (ok.stdout.as_str(), ok.code),
        ("✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ Checks valid\n", 0)
    );
    // 128 calls and `Wide` are 129.
    let over = check("over.velme", 128);
    assert_eq!(over.stdout, "✓ Parsed\n✓ Types valid\n");
    // `VL06xx` is exit status 3 by `tooling/40` §4, though this one is found before anything runs.
    assert_eq!(over.code, 3);
    assert!(
        over.stderr
            .contains("Error: Too many goals were called while running `Wide`.  [VL0605]"),
        "{}",
        over.stderr
    );
    assert!(
        over.stderr
            .contains("it would run 129 goals, counting itself, but its limit is 128")
    );
    // Nothing ran, and nothing was built or stored.
    assert!(!dir.join(".velme").exists());
}
