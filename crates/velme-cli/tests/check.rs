//! `velme check` as a learner runs it: output, `--json` and exit codes (`tooling/40` §3–4, `reference/90` AC-ERR-03).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;

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
    let out = Command::new(env!("CARGO_BIN_EXE_velme"))
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
        ("✓ Parsed\n", "", 0)
    );
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
    assert_eq!((run.stdout.as_str(), run.code), ("✓ Parsed\n", 0));
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
    let dir = repo_root().join("tests/golden/parser/reject");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("reject dir")
        .map(|e| e.expect("entry").path())
        .collect();
    files.sort();
    for file in files.iter().filter(|f| f.extension().is_some_and(|e| e == "velme")) {
        let rel = format!(
            "tests/golden/parser/reject/{}",
            file.file_name().and_then(|n| n.to_str()).expect("name")
        );
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

/// The syntax part of AC-CMP-04: `--json` for every golden error file matches its snapshot.
#[test]
fn ac_cmp_04_json_for_every_golden_error_file() {
    insta::glob!("../../../tests/golden/parser/reject", "*.velme", |path| {
        let name = path.file_name().and_then(|n| n.to_str()).expect("UTF-8 file name");
        let rel = format!("tests/golden/parser/reject/{name}");
        let run = velme(&["check", "--json", &rel]);
        let json = json(&run);
        let expected = if json["status"] == "ok" { 0 } else { 1 };
        assert_eq!(run.code, expected, "{rel}");
        insta::assert_json_snapshot!(json);
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
