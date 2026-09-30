//! `velme build --provider external` through the binary (`tooling/40` §2.1, R-CLI-13, `compiler/22` R-SYNTH-28,
//! R-SYNTH-29, `tooling/41` R-SEC-05, R-SEC-12, R-SEC-13): where the command comes from, what the child sees, and what
//! the learner is told. The test backend is `velme-test-backend` of `velme-test-support`.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use velme_builtins::BUILTINS_VERSION;
use velme_ir::IR_VERSION;
use velme_test_support::{backend_binary, backend_command};

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    check:
        - result == n * 2
    examples:
        - Double(2) == 4
";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

/// A project with `SOURCE`, and a directory of backend replies holding a good `Double`.
fn project(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-external").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("replies")).expect("project directory");
    fs::write(dir.join("game.velme"), SOURCE).expect("source");
    let number = json!({"t": "Number"});
    let double = json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "Double", "types": {},
        "inputs": [["n", number]], "output": number,
        "body": {"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                 "right": {"kind": "literal", "type": number, "value": 2}}});
    fs::write(dir.join("replies/Double.json"), double.to_string()).expect("reply");
    let replies = dir.join("replies");
    (dir, replies)
}

fn velme(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Out {
    let mut command = Command::new(env!("CARGO_BIN_EXE_velme"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_COMMAND")
        .envs(envs.iter().copied());
    let out = command.output().expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn dir_arg(replies: &Path) -> String {
    replies.to_str().expect("utf-8").to_owned()
}

/// Every file under `dir`, as text where it is, for grepping.
fn all_text(dir: &Path, found: &mut String) {
    for entry in fs::read_dir(dir).expect("directory").filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            all_text(&path, found);
        } else if let Ok(text) = fs::read_to_string(&path) {
            found.push_str(&text);
        }
    }
}

/// The child gets the user's environment minus every variable ending in `_API_KEY`, whatever its case and whether or not
/// Velme reads it, and none of them lands in any output or file (AC-SYNTH-18, R-SEC-13, AC-SEC-05). The project's
/// `velme.toml` naming the command is the config file's business (M6).
#[test]
fn ac_synth_18_the_command_gets_no_api_key_variables() {
    let (dir, replies) = project("env");
    let dump = dir.join("env.txt");
    let command = backend_command(&["--dir", &dir_arg(&replies), "--env-dump", dump.to_str().expect("utf-8")]);
    let out = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            &command,
        ],
        &[
            ("VELME_API_KEY", "sk-SENTINEL-ONE"),
            ("ANTHROPIC_API_KEY", "sk-SENTINEL-TWO"),
            ("openai_api_key", "sk-SENTINEL-THREE"),
            ("My_Api_Key", "sk-SENTINEL-FOUR"),
            ("KEEP_ME", "kept-value"),
        ],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let seen = fs::read_to_string(&dump).expect("the child's environment");
    assert!(
        seen.contains("KEEP_ME=kept-value"),
        "the rest of the environment is kept"
    );
    for name in [
        "VELME_API_KEY",
        "ANTHROPIC_API_KEY",
        "openai_api_key",
        "My_Api_Key",
        "SENTINEL",
    ] {
        assert!(!seen.contains(name), "the child sees {name}");
    }
    let mut everything = format!("{}{}", out.stdout, out.stderr);
    all_text(&dir, &mut everything);
    assert!(!everything.contains("SENTINEL"), "a key reached an output or a file");
}

/// A relative path as the command is `VL0902` before any process starts; a bare name found on `PATH` runs; one that only
/// the project directory or `.` holds is not found (AC-SYNTH-36, R-SYNTH-29, D-50).
#[test]
fn ac_synth_36_a_relative_command_is_never_run_and_a_bare_name_comes_from_path() {
    let (dir, replies) = project("relative");
    let marker = dir.join("ran");
    let script = dir.join("evil.sh");
    fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).expect("script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    for command in ["./evil.sh", "evil.sh --x", "bin/impl", "../impl"] {
        let out = velme(
            &dir,
            &[
                "build",
                "game.velme",
                "--provider",
                "external",
                "--external-command",
                command,
            ],
            &[],
        );
        let shown = format!("{}{}", out.stdout, out.stderr);
        let relative = command.starts_with("./") || command.starts_with("bin/") || command.starts_with("..");
        if relative {
            assert!(shown.contains("VL0902"), "{command}: {shown}");
        }
        assert!(!marker.exists(), "{command} was run");
    }
    // A program that only the project directory, or `.`, holds is not on the path (VL0405, never run).
    #[cfg(unix)]
    {
        let path = format!("{}:.:{}", dir.display(), std::env::var("PATH").unwrap_or_default());
        let out = velme(
            &dir,
            &[
                "build",
                "game.velme",
                "--provider",
                "external",
                "--external-command",
                "evil.sh",
            ],
            &[("PATH", &path)],
        );
        assert!(
            format!("{}{}", out.stdout, out.stderr).contains("VL0405"),
            "{}{}",
            out.stdout,
            out.stderr
        );
        assert!(!marker.exists(), "a program of the project directory was run");
        // Nor one in a subdirectory of it.
        let sub = dir.join("tools");
        fs::create_dir_all(&sub).expect("subdirectory");
        fs::copy(&script, sub.join("evil.sh")).expect("copied");
        let path = format!("{}:{}", sub.display(), std::env::var("PATH").unwrap_or_default());
        let out = velme(
            &dir,
            &[
                "build",
                "game.velme",
                "--provider",
                "external",
                "--external-command",
                "evil.sh",
            ],
            &[("PATH", &path)],
        );
        assert!(
            format!("{}{}", out.stdout, out.stderr).contains("VL0405"),
            "{}{}",
            out.stdout,
            out.stderr
        );
        assert!(!marker.exists(), "a program under the project directory was run");
    }
    // A bare name in a directory that is on `PATH`, and not the project's, runs.
    let bin_dir = backend_binary().parent().expect("a directory").to_path_buf();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());
    let command = format!("velme-test-backend --dir '{}'", replies.display());
    let out = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            &command,
        ],
        &[("PATH", &path)],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
}

/// The command can come from `VELME_EXTERNAL_COMMAND` too; with neither it is `VL0405` naming both; a build that needs
/// no provider doesn't mind (R-CLI-13, R-CLI-12).
#[test]
fn the_command_comes_from_the_flag_or_the_environment() {
    let (dir, replies) = project("sources");
    let none = velme(&dir, &["build", "game.velme", "--provider", "external"], &[]);
    let shown = format!("{}{}", none.stdout, none.stderr);
    assert!(
        shown.contains("VL0405") && shown.contains("VELME_EXTERNAL_COMMAND"),
        "{shown}"
    );
    let command = backend_command(&["--dir", &dir_arg(&replies)]);
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "external"],
        &[("VELME_EXTERNAL_COMMAND", &command)],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    // The notice names the command, once, before the first contact; the cached build prints none (R-SEC-12).
    let notice = "Sending your plans, types, checks and examples to the command";
    assert_eq!(out.stderr.matches(notice).count(), 1, "{}", out.stderr);
    let again = velme(
        &dir,
        &["build", "game.velme", "--provider", "external"],
        &[("VELME_EXTERNAL_COMMAND", &command)],
    );
    assert_eq!(again.code, 0);
    assert!(!again.stderr.contains(notice), "{}", again.stderr);
    // Nothing is fresh any more, so a provider is needed.
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let empty = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            "   ",
        ],
        &[],
    );
    assert!(format!("{}{}", empty.stdout, empty.stderr).contains("VL0405"));
    let broken = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            "'open",
        ],
        &[],
    );
    assert!(format!("{}{}", broken.stdout, broken.stderr).contains("VL0902"));
}

/// A backend that answers `{"pending"}` leaves the goal waiting and the build failed with exit code 2; the next build,
/// once the backend has the answer, succeeds (AC-SYNTH-31, D-45).
#[test]
fn a_pending_answer_exits_2_and_the_next_build_asks_again() {
    let (dir, replies) = project("pending");
    let good = fs::read_to_string(replies.join("Double.json")).expect("reply");
    fs::write(replies.join("Double.json"), r#"{"pending": "ticket 42"}"#).expect("pending");
    let command = backend_command(&["--dir", &dir_arg(&replies)]);
    let args = [
        "build",
        "game.velme",
        "--provider",
        "external",
        "--external-command",
        command.as_str(),
    ];
    let first = velme(&dir, &args, &[]);
    assert_eq!(first.code, 2, "{}{}", first.stdout, first.stderr);
    let shown = format!("{}{}", first.stdout, first.stderr);
    assert!(shown.contains("VL0408") && shown.contains("ticket 42"), "{shown}");
    assert!(!dir.join("velme.lock").exists());
    fs::write(replies.join("Double.json"), good).expect("answer");
    let second = velme(&dir, &args, &[]);
    assert_eq!(second.code, 0, "{}{}", second.stdout, second.stderr);
    assert!(dir.join("velme.lock").is_file());
}

/// A backend failure is `VL0406` with exit code 2 and its stderr as a note; `external` defaults to no retries, so the
/// failing backend is asked once (R-SYNTH-28, R-SYNTH-30).
#[test]
fn a_failing_backend_is_vl0406_and_is_asked_once() {
    let (dir, replies) = project("failing");
    let log = dir.join("log.txt");
    let command = backend_command(&[
        "--dir",
        &dir_arg(&replies),
        "--log",
        log.to_str().expect("utf-8"),
        "--mode",
        "exit",
    ]);
    let out = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            &command,
        ],
        &[],
    );
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    let shown = format!("{}{}", out.stdout, out.stderr);
    assert!(
        shown.contains("VL0406") && shown.contains("velme-test-support"),
        "{shown}"
    );
    // Human mode shows the raw stderr escaped, never an ESC byte (AC-CLI-14 is M6; here nothing raw gets through).
    assert!(!shown.contains('\u{1b}'), "a raw ESC reached the terminal");
    assert_eq!(
        fs::read_to_string(&log).expect("log").lines().count(),
        2,
        "describe, then one request"
    );
}

/// `velme run --input` never reaches a provider: the command isn't started, and nothing of the input is sent anywhere
/// (AC-SEC-06, R-SEC-08).
#[test]
fn ac_sec_06_a_run_with_input_makes_no_provider_request() {
    let (dir, replies) = project("run-input");
    let log = dir.join("log.txt");
    let capture = dir.join("capture.json");
    let command = backend_command(&[
        "--dir",
        &dir_arg(&replies),
        "--log",
        log.to_str().expect("utf-8"),
        "--capture",
        capture.to_str().expect("utf-8"),
    ]);
    let envs = [("VELME_EXTERNAL_COMMAND", command.as_str())];
    let built = velme(&dir, &["build", "game.velme", "--provider", "external"], &envs);
    assert_eq!(built.code, 0, "{}{}", built.stdout, built.stderr);
    let (logged, captured) = (fs::read(&log).expect("log"), fs::read(&capture).expect("capture"));
    fs::write(dir.join("input.json"), r#"{"n": 987654321}"#).expect("input");
    let out = velme(
        &dir,
        &["run", "game.velme", "--goal", "Double", "--input", "input.json"],
        &envs,
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("1975308642"), "{}", out.stdout);
    assert_eq!(fs::read(&log).expect("log"), logged, "the command was started again");
    assert_eq!(fs::read(&capture).expect("capture"), captured);
    assert!(!String::from_utf8_lossy(&captured).contains("987654321"));
}

/// The `ollama` provider needs a model and no key: without one the goal is `VL0405` and no server is contacted
/// (R-CLI-12, R-SYNTH-24).
#[test]
fn the_ollama_provider_needs_a_model_and_no_key() {
    let (dir, _) = project("ollama");
    let out = velme(&dir, &["build", "game.velme", "--provider", "ollama"], &[]);
    let shown = format!("{}{}", out.stdout, out.stderr);
    assert_eq!(out.code, 2, "{shown}");
    assert!(shown.contains("VL0405") && shown.contains("--model"), "{shown}");
    assert!(!shown.contains("API"), "no key is asked for: {shown}");
    assert!(!out.stderr.contains("Sending"), "nothing is sent: {}", out.stderr);
}

/// A replayed `external` build asks as often as the recorded one did: with no retries, so a rejected reply that was
/// recorded once replays once and the goal fails the same way, rather than asking for an attempt no fixture holds
/// (R-SYNTH-30, R-SYNTH-43).
#[test]
fn a_replayed_external_build_reproduces_the_recorded_one() {
    let (dir, replies) = project("replay-retries");
    // A reply that fails the goal's example.
    let wrong = fs::read_to_string(replies.join("Double.json"))
        .expect("reply")
        .replace("\"value\":2", "\"value\":3");
    fs::write(replies.join("Double.json"), wrong).expect("wrong reply");
    let command = backend_command(&["--dir", &dir_arg(&replies)]);
    let record = Command::new(env!("CARGO_BIN_EXE_velme"))
        .args([
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            &command,
        ])
        .current_dir(&dir)
        .env("VELME_SYNTH_RECORD", "1")
        .output()
        .expect("velme runs");
    let recorded = format!(
        "{}{}",
        String::from_utf8_lossy(&record.stdout),
        String::from_utf8_lossy(&record.stderr)
    );
    assert_eq!(record.status.code(), Some(2), "{recorded}");
    assert!(recorded.contains("VL0403"), "{recorded}");
    let replayed = velme(&dir, &["build", "game.velme", "--provider", "replay"], &[]);
    assert_eq!(replayed.code, 2, "{}{}", replayed.stdout, replayed.stderr);
    let shown = format!("{}{}", replayed.stdout, replayed.stderr);
    assert!(shown.contains("VL0403") && !shown.contains("VL0404"), "{shown}");
}

/// The notice of what is sent names the user's model and command, which reach the terminal escaped: no raw ESC byte or
/// bidi control gets through (R-SEC-12, D-47).
#[test]
fn the_notice_shows_the_model_and_command_escaped() {
    let (dir, _) = project("notice-escape");
    let model = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "ollama",
            "--model",
            "m\u{1b}[31mx\u{202e}y",
        ],
        &[],
    );
    let program = dir
        .join("no-such-backend\u{1b}[31m")
        .to_string_lossy()
        .replace(' ', "\\ ");
    let command = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-command",
            &program,
        ],
        &[],
    );
    for (name, out) in [("model", &model), ("command", &command)] {
        assert!(out.stderr.contains("Sending your plans"), "{name}: {}", out.stderr);
        assert!(out.stderr.contains("\\u{1b}"), "{name}: {}", out.stderr);
        assert!(
            !out.stderr.contains('\u{1b}') && !out.stderr.contains('\u{202e}'),
            "{name}: {:?}",
            out.stderr
        );
    }
    assert!(model.stderr.contains("\\u{202e}"), "{}", model.stderr);
}
