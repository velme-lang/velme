//! The CLI contract (`tooling/40`): inputs, `velme test`, config files and their errors, bad flags, colour, `gc` and
//! `cache clean`, and the artifact criteria that need a hand-written lock. Everything runs the binary on the committed
//! fixture projects or copies of them, and none of it has a provider to reach.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use velme_runtime::{Entry, Lock, Store};
use velme_test_support::schema::assert_cli_envelope;
use velme_test_support::workload::user_cache;
use velme_test_support::{install, program, read, repo};

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// `velme` in `dir` with `envs` set, and the user's own provider, colour and config settings removed: the user-level config
/// points at a directory inside `dir` and the module cache at [`user_cache`] beside it, outside the project as R-SBX-20
/// asks, so a test never reads the developer's own (D-135).
fn velme_env(dir: &Path, args: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> Run {
    let home = dir.join("no-home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_velme"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_SCRIPT")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_URL")
        .env_remove("VELME_OLLAMA_URL")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_CACHE_HOME", user_cache(dir))
        .env("APPDATA", &home)
        .env("LOCALAPPDATA", user_cache(dir))
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("velme runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin)
        .expect("stdin written");
    let out = child.wait_with_output().expect("velme exits");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is UTF-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn velme(dir: &Path, args: &[&str]) -> Run {
    velme_env(dir, args, &[], b"")
}

/// The `--json` envelope `run` printed, which must be valid against the published schema (AC-CLI-12).
fn parsed(run: &Run) -> Value {
    let value: Value = serde_json::from_str(&run.stdout).expect("--json prints one JSON document");
    assert_cli_envelope(&value);
    value
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

/// A copy of the committed fixture project `name`, in a fresh directory for `test` with an empty [`user_cache`].
fn copy(name: &str, test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli").join(test);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(user_cache(&dir));
    fs::create_dir_all(&dir).expect("scratch directory");
    for (file, bytes) in tree(&repo(&format!("tests/fixtures/run/{name}"))) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        fs::write(path, bytes).expect("copied");
    }
    dir
}

const ADD: &str = "add.velme";
const DOUBLE: &str = "double_then_add_one.velme";
const PLAYER: &str = "player_summary.velme";

fn codes(envelope: &Value) -> Vec<String> {
    let mut all: Vec<String> = envelope["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| d["code"].as_str().expect("a code").to_owned())
        .collect();
    for result in envelope["results"].as_array().expect("results") {
        for d in result["diagnostics"].as_array().expect("diagnostics") {
            all.push(d["code"].as_str().expect("a code").to_owned());
        }
    }
    all
}

// ---- check, inputs, test ----

/// `velme check` on a valid, locked file prints the five ✓ lines and exits 0; on a type error it exits 1 with the code and
/// a source caret (AC-CLI-01).
#[test]
fn ac_cli_01_check_prints_five_lines_and_a_type_error_exits_1_with_a_caret() {
    let dir = copy("add", "ac_cli_01");
    let run = velme(&dir, &["check", ADD]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        (
            "✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ IR valid        (1 goal locked)\n✓ Checks valid\n",
            "",
            0
        )
    );
    fs::write(
        dir.join("bad.velme"),
        "language: velme/0.1\n\ngoal Show(p: Playr) -> Number:\n    plan: \"Show it.\"\n",
    )
    .expect("source");
    let run = velme(&dir, &["check", "bad.velme"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0201]"), "{}", run.stderr);
    assert!(run.stderr.contains("bad.velme:3:"), "{}", run.stderr);
    assert!(run.stderr.contains("───"), "a caret span: {}", run.stderr);
    assert!(!run.stdout.contains("Checks valid"), "{}", run.stdout);
}

/// `--arg` overrides a key of `--input`; a missing record field exits 64 with `VL0902` naming the field and its type
/// (AC-CLI-04, R-CLI-07).
#[test]
fn ac_cli_04_arg_overrides_input_and_a_missing_field_is_vl0902() {
    let dir = copy("add", "ac_cli_04");
    fs::write(dir.join("in.json"), r#"{"a": 1, "b": 2}"#).expect("input");
    let run = velme(&dir, &["run", ADD, "--goal", "Add", "--input", "in.json", "--json"]);
    assert_eq!(parsed(&run)["results"][0]["result"], 3);
    let run = velme(
        &dir,
        &[
            "run", ADD, "--goal", "Add", "--input", "in.json", "--arg", "a=10", "--json",
        ],
    );
    assert_eq!(parsed(&run)["results"][0]["result"], 12);
    let run = velme_env(
        &dir,
        &["run", ADD, "--goal", "Add", "--input", "-", "--arg", "b=5", "--json"],
        &[],
        br#"{"a": 1}"#,
    );
    assert_eq!(parsed(&run)["results"][0]["result"], 6);

    let dir = copy("player_summary", "ac_cli_04_record");
    let run = velme(
        &dir,
        &[
            "run",
            PLAYER,
            "--goal",
            "FindBadge",
            "--arg",
            r#"player={"name":"Lina","score":9}"#,
        ],
    );
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0902]"), "{}", run.stderr);
    assert!(
        run.stderr.contains("`player`") && run.stderr.contains("Player") && run.stderr.contains("`jump_height`"),
        "{}",
        run.stderr
    );
}

/// The call lines of a run come in source order, every time, for a goal whose calls run at the same time (AC-CLI-06, D-9).
#[test]
fn ac_cli_06_call_lines_keep_source_order_across_100_runs() {
    let dir = copy("level_summary", "ac_cli_06");
    let args = [
        "run",
        "level_summary.velme",
        "--goal",
        "CreateLevelSummary",
        "--arg",
        r#"level={"enemy_count":7,"treasure_count":3,"base_score":120}"#,
    ];
    let first = velme(&dir, &args);
    assert_eq!(first.code, 0, "{}", first.stderr);
    let lines: Vec<&str> = first.stdout.lines().take(6).collect();
    let names: Vec<&str> = lines
        .iter()
        .map(|l| l.split_whitespace().next().expect("a name"))
        .collect();
    assert_eq!(
        names,
        [
            "CountEnemies",
            "CountTreasures",
            "CalculateLevelScore",
            "RateDifficulty",
            "CalculateReward",
            "CreateLevelSummary"
        ]
    );
    for _ in 0..100 {
        assert_eq!(velme(&dir, &args).stdout, first.stdout);
    }
}

/// `velme test` runs the `examples:` first, for a goal with calls as for a leaf, and reports a failing example with what it
/// was given, expected and got; the generated inputs follow only when the examples pass (AC-CLI-10, D-107).
#[test]
fn ac_cli_10_test_runs_examples_before_generated_inputs_for_leaf_and_composite_goals() {
    let dir = copy("double_then_add_one", "ac_cli_10");
    let run = velme(&dir, &["test", DOUBLE]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert!(
        lines[0].starts_with("Double  ✓ 0 examples, ") && lines[0].contains(" generated inputs"),
        "{}",
        run.stdout
    );
    // The composite goal: its one example and generated inputs, through the goals it calls.
    assert!(lines[2].starts_with("Main  ✓ 1 example, "), "{}", run.stdout);

    // A leaf that fails its examples: each failing example is reported with what it was given, expected and got, and the
    // generated inputs never run, so nothing but the three examples and their checks is in the report.
    let broken = copy("add_broken", "ac_cli_10_broken");
    let run = velme(&broken, &["test", ADD, "--json"]);
    assert_eq!(run.code, 3, "{}", run.stderr);
    let envelope = parsed(&run);
    assert_eq!(
        codes(&envelope),
        ["VL0502", "VL0501", "VL0502", "VL0501", "VL0502", "VL0501"]
    );
    let message = envelope["results"][0]["diagnostics"][0]["message"]
        .as_str()
        .expect("a message");
    assert_eq!(message, "For Add(2, 3), `Add` gave 6 but the example expects 5.");
    let run = velme(&broken, &["test", ADD]);
    assert!(run.stderr.contains("[VL0502]"), "{}", run.stderr);

    // A goal with calls runs through the goals it calls: a wrong child fails the composite's run.
    let source = read(&dir.join(DOUBLE));
    let number = serde_json::json!({"t": "Number"});
    let input = serde_json::json!({"kind": "input", "name": "x"});
    let literal = |n: i64| serde_json::json!({"kind": "literal", "type": number, "value": n});
    let ir = serde_json::json!({
        "ir_version": "0.1", "builtins_version": "0.1", "goal": "AddOne", "types": {},
        "inputs": [["x", number]], "output": number,
        "body": {"kind": "binary", "op": "add", "left": input, "right": literal(2)}
    });
    install(&dir, DOUBLE, &program(&source), &ir.to_string());
    let run = velme(&dir, &["test", DOUBLE, "--goal", "Main", "--json"]);
    assert_eq!(run.code, 3, "{}", run.stderr);
    let envelope = parsed(&run);
    assert_eq!(codes(&envelope), ["VL0501"]);
    let message = envelope["results"][0]["diagnostics"][0]["message"]
        .as_str()
        .expect("a message");
    assert!(message.contains("`Main` failed because `AddOne` failed"), "{message}");
    // The wrong child fails its own check on a generated input, which is what the leaf's test reports.
    let run = velme(&dir, &["test", DOUBLE, "--goal", "AddOne", "--json"]);
    assert_eq!(run.code, 3, "{}", run.stderr);
    assert_eq!(codes(&parsed(&run)), ["VL0501"]);
    let run = velme(&dir, &["test", DOUBLE, "--goal", "Main"]);
    assert!(run.stdout.starts_with("Main  ✗\n"), "{}", run.stdout);
    assert!(run.stderr.contains("[VL0501]"), "{}", run.stderr);
}

/// `run --json` for a goal with calls has one result for the requested goal, with `calls` in source order (AC-CLI-26).
#[test]
fn ac_cli_26_a_composite_run_has_one_result_with_its_calls_in_source_order() {
    let dir = copy("player_summary", "ac_cli_26");
    let run = velme(
        &dir,
        &[
            "run",
            PLAYER,
            "--goal",
            "BuildPlayerSummary",
            "--arg",
            r#"player={"name":"Lina","jump_height":3,"score":820}"#,
            "--json",
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    let envelope = parsed(&run);
    let results = envelope["results"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["goal"], "BuildPlayerSummary");
    assert_eq!(
        results[0]["calls"],
        serde_json::json!([
            {"binding": "score", "goal": "CalculateScore", "status": "ok"},
            {"binding": "badge", "goal": "FindBadge", "status": "ok"}
        ])
    );
    // A leaf has no calls to list.
    let run = velme(
        &dir,
        &[
            "run",
            PLAYER,
            "--goal",
            "CalculateScore",
            "--arg",
            r#"player={"name":"L","jump_height":1,"score":5}"#,
            "--json",
        ],
    );
    assert!(parsed(&run)["results"][0].get("calls").is_none());
}

// ---- input limits (AC-SEC-08) ----

/// Input JSON above 16 MiB, or nested deeper than 128, is `VL0902` before it is decoded (AC-SEC-08, R-CLI-07, T-9).
#[test]
fn ac_sec_08_oversized_or_over_deep_input_is_rejected_before_decoding() {
    let dir = copy("add", "ac_sec_08");
    let big = format!(
        "{{\"a\": 1, \"b\": 2, \"pad\": \"{}\"}}",
        "x".repeat(16 * 1024 * 1024 + 1)
    );
    fs::write(dir.join("big.json"), &big).expect("input");
    let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    fs::write(dir.join("deep.json"), format!("{{\"a\": {deep}, \"b\": 2}}")).expect("input");
    for (file, wanted) in [("big.json", "too big"), ("deep.json", "deeper than 128 levels")] {
        let run = velme(&dir, &["run", ADD, "--goal", "Add", "--input", file, "--json"]);
        assert_eq!(run.code, 64, "{file}: {}", run.stderr);
        let envelope = parsed(&run);
        assert_eq!(codes(&envelope), ["VL0902"], "{file}");
        let d = envelope["diagnostics"]
            .as_array()
            .and_then(|d| d.first())
            .unwrap_or(&envelope["results"][0]["diagnostics"][0]);
        assert!(d.to_string().contains(wanted), "{file}: {d}");
    }
    // Standard input is held to the same limit.
    let run = velme_env(
        &dir,
        &["run", ADD, "--goal", "Add", "--input", "-", "--json"],
        &[],
        big.as_bytes(),
    );
    assert_eq!(run.code, 64);
    assert_eq!(codes(&parsed(&run)), ["VL0902"]);
}

// ---- config files ----

fn write_project_config(dir: &Path, text: &str) {
    fs::write(dir.join("velme.toml"), text).expect("velme.toml");
}

fn write_user_config(dir: &Path, text: &str) -> PathBuf {
    let file = dir.join("no-home").join("velme").join("config.toml");
    fs::create_dir_all(file.parent().expect("a parent")).expect("directory");
    fs::write(&file, text).expect("user config");
    file
}

/// An unknown key in `velme.toml` fails with `VL0902`, whichever command reads the file (AC-CLI-09, R-CLI-11, R-CLI-25).
#[test]
fn ac_cli_09_an_unknown_key_in_velme_toml_is_vl0902() {
    let dir = copy("add", "ac_cli_09");
    for text in ["nope = 1\n", "[synthesis]\nnope = 1\n", "[project]\ncolor = 3\n"] {
        write_project_config(&dir, text);
        for args in [
            &["check", ADD][..],
            &["run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2"],
            &["test", ADD],
            &["explain", ADD, "--goal", "Add"],
            &["artifact", ADD, "--goal", "Add"],
            &["build", ADD, "--offline"],
        ] {
            let run = velme(&dir, args);
            assert_eq!(run.code, 64, "{text:?} {args:?}: {}", run.stderr);
            assert!(
                run.stderr.contains("[VL0902]") && run.stderr.contains("velme.toml"),
                "{}",
                run.stderr
            );
            assert!(run.stderr.contains("unknown key"), "{}", run.stderr);
        }
    }
}

/// `[artifacts] dir = "/etc"` and `replay_dir = "../outside"` each fail with `VL0902` (AC-CLI-15, R-CLI-18).
#[test]
fn ac_cli_15_artifact_and_replay_paths_must_stay_inside_the_project() {
    let dir = copy("add", "ac_cli_15");
    for (text, key) in [
        ("[artifacts]\ndir = \"/etc\"\n", "artifacts.dir"),
        ("[synthesis]\nreplay_dir = \"../outside\"\n", "synthesis.replay_dir"),
        ("[artifacts]\ndir = \"a/../../b\"\n", "artifacts.dir"),
    ] {
        write_project_config(&dir, text);
        let run = velme(&dir, &["check", ADD, "--json"]);
        assert_eq!(run.code, 64, "{text}: {}", run.stderr);
        let envelope = parsed(&run);
        assert_eq!(codes(&envelope), ["VL0902"]);
        let message = envelope["diagnostics"][0]["message"].as_str().expect("a message");
        assert!(message.contains(&format!("`{key}` in `velme.toml`")), "{message}");
        assert!(
            message.contains("should be a relative path inside the project, but got"),
            "{message}"
        );
    }
    write_project_config(&dir, "[synthesis]\nreplay_dir = \"tests/fixtures/synth\"\n");
    assert_eq!(velme(&dir, &["check", ADD]).code, 0);
}

/// A service URL in a project's `velme.toml`, an unknown key in the user-level file, and a wrong type or out-of-range value
/// in either are `VL0902` in the "should be … but got …" wording; an unreadable config or CA file is `VL0901`; and the
/// user-level file is read from `$XDG_CONFIG_HOME` (AC-CLI-22, R-CLI-25, D-105).
#[test]
fn ac_cli_22_config_errors_are_vl0902_with_the_should_be_wording_and_unreadable_files_vl0901() {
    let dir = copy("add", "ac_cli_22");
    let message = |run: &Run| {
        let envelope = parsed(run);
        envelope["diagnostics"][0]["message"]
            .as_str()
            .expect("a message")
            .to_owned()
    };
    // The project's file.
    for (text, expected) in [
        (
            "[synthesis]\nollama_url = \"http://127.0.0.1:1\"\n",
            "`synthesis.ollama_url` in `velme.toml` should be left out of a project's file, but got a value.",
        ),
        (
            "[synthesis]\nexternal_url = \"https://x.example\"\n",
            "`synthesis.external_url` in `velme.toml` should be left out of a project's file, but got a value.",
        ),
        (
            "[synthesis]\nmax_retries = 4\n",
            "`synthesis.max_retries` in `velme.toml` should be a whole number from 0 to 3, but got 4.",
        ),
        (
            "[synthesis]\nmodel = 5\n",
            "`synthesis.model` in `velme.toml` should be a non-empty string, but got 5.",
        ),
        (
            "[synthesis]\nprompt_cache = \"yes\"\n",
            "`synthesis.prompt_cache` in `velme.toml` should be true or false, but got \"yes\".",
        ),
        ("[synthesis\n", "`line 1` in `velme.toml` should be valid TOML, but got"),
    ] {
        write_project_config(&dir, text);
        let run = velme(&dir, &["check", ADD, "--json"]);
        assert_eq!(run.code, 64, "{text}: {}", run.stderr);
        assert!(message(&run).starts_with(expected), "{text}: {}", message(&run));
    }
    write_project_config(&dir, "");
    // The user-level file, found under $XDG_CONFIG_HOME.
    let user = write_user_config(&dir, "[synthesis]\nmodel = \"x\"\n");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(
        message(&run).contains("`synthesis.model` in `") && message(&run).contains("config.toml"),
        "{}",
        message(&run)
    );
    assert!(
        message(&run).ends_with("should be a setting Velme knows, but got an unknown key."),
        "{}",
        message(&run)
    );
    fs::write(&user, "[synthesis]\nmax_output_tokens = 0\n").expect("user config");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert!(
        message(&run).contains("should be a whole number from 1 to 1000000, but got 0."),
        "{}",
        message(&run)
    );
    fs::write(&user, "[synthesis]\nallowed_models = \"gpt\"\n").expect("user config");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert!(
        message(&run).contains("should be a list of non-empty strings, but got \"gpt\"."),
        "{}",
        message(&run)
    );
    fs::write(&user, "[synthesis]\nexternal_ca_file = \"relative.pem\"\n").expect("user config");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert!(
        message(&run).contains("should be an absolute path"),
        "{}",
        message(&run)
    );
    // Both files are wrong: both are reported.
    write_project_config(&dir, "nope = 1\n");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert_eq!(parsed(&run)["diagnostics"].as_array().expect("diagnostics").len(), 2);
    write_project_config(&dir, "");

    // An unreadable config is VL0901: a directory where the user-level file should be, and a `--config` that isn't there.
    fs::remove_file(&user).expect("removed");
    fs::create_dir(&user).expect("a directory in its place");
    let run = velme(&dir, &["check", ADD, "--json"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert_eq!(codes(&parsed(&run)), ["VL0901"]);
    fs::remove_dir(&user).expect("removed");
    let run = velme(&dir, &["check", ADD, "--config", "missing.toml", "--json"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert_eq!(codes(&parsed(&run)), ["VL0901"]);
    // A CA file that can't be read, or holds no certificate, is VL0901 too (the external provider is what reads it).
    for (ca, name) in [
        (dir.join("nothing.pem"), "unreadable"),
        (dir.join("empty.pem"), "empty"),
    ] {
        fs::write(dir.join("empty.pem"), "not a certificate\n").expect("pem");
        write_user_config(
            &dir,
            &format!("[synthesis]\nexternal_ca_file = {:?}\n", ca.to_string_lossy()),
        );
        let run = velme(
            &dir,
            &[
                "build",
                ADD,
                "--provider",
                "external",
                "--external-url",
                "https://127.0.0.1:9",
                "--json",
            ],
        );
        assert_eq!(run.code, 64, "{name}: {}", run.stderr);
        assert_eq!(codes(&parsed(&run)), ["VL0901"], "{name}");
    }
}

/// `--config PATH` gives the project's settings and does not move the project root (AC-CLI-22, R-CLI-25, D-105).
#[test]
fn a_config_flag_supplies_settings_and_leaves_the_project_root_alone() {
    let dir = copy("add", "config_flag");
    fs::write(dir.join("elsewhere.toml"), "nope = 1\n").expect("config");
    let run = velme(&dir, &["check", ADD, "--config", "elsewhere.toml", "--json"]);
    assert_eq!(run.code, 64);
    assert!(
        parsed(&run)["diagnostics"][0]["message"]
            .as_str()
            .expect("message")
            .contains("`nope` in `elsewhere.toml`")
    );
    fs::write(dir.join("elsewhere.toml"), "[synthesis]\nprovider = \"replay\"\n").expect("config");
    let run = velme(
        &dir,
        &[
            "run",
            ADD,
            "--goal",
            "Add",
            "--arg",
            "a=1",
            "--arg",
            "b=2",
            "--config",
            "elsewhere.toml",
        ],
    );
    assert_eq!(
        run.code, 0,
        "the lock and store are still the file's own: {}",
        run.stderr
    );
}

// ---- bad flags ----

/// `--provider` on `check`, an unknown `--backend` value and an invalid flag value exit 64 with `VL0902`, inside the envelope under
/// `--json`; `-q` drops progress lines and nothing else (AC-CLI-24, R-CLI-14, R-CLI-21, D-108, D-117).
#[test]
fn ac_cli_24_bad_flags_are_vl0902_in_the_envelope_and_quiet_drops_only_progress() {
    let dir = copy("add", "ac_cli_24");
    let bad: [&[&str]; 8] = [
        &["check", ADD, "--provider", "replay"],
        &["run", ADD, "--goal", "Add", "--backend", "native"],
        &["run", ADD, "--goal", "Add", "--jobs", "0"],
        &["check", ADD, "--color", "pink"],
        &["check", ADD, "--frobnicate"],
        &["run", ADD, "--goal", "Add", "--provider", "replay"],
        &["test", ADD, "--build", "--locked"],
        &["frobnicate", ADD],
    ];
    for args in bad {
        let run = velme(&dir, args);
        assert_eq!(run.code, 64, "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains("[VL0902]"), "{args:?}: {}", run.stderr);
        assert!(run.stderr.starts_with("usage: velme"), "{args:?}: {}", run.stderr);
        let mut json_args = args.to_vec();
        json_args.push("--json");
        let run = velme(&dir, &json_args);
        assert_eq!(run.code, 64, "{json_args:?}");
        assert_eq!(run.stderr, "", "{json_args:?}");
        let envelope = parsed(&run);
        assert_eq!(envelope["status"], "failed");
        assert_eq!(codes(&envelope), ["VL0902"], "{json_args:?}");
    }
    let run = velme(&dir, &["run", ADD, "--goal", "Add", "--backend", "native", "--json"]);
    assert_eq!(
        parsed(&run)["diagnostics"][0]["message"],
        "Input `--backend` should be interp, wasm or auto, but got `native`."
    );

    // `-q` drops the progress lines and keeps the result and the diagnostics.
    let args = ["run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2"];
    let loud = velme(&dir, &args);
    assert!(loud.stdout.starts_with("Add  ✓\n\nResult:\n3\n"), "{}", loud.stdout);
    let quiet = velme(&dir, &[&args[..], &["-q"]].concat());
    assert_eq!((quiet.stdout.as_str(), quiet.code), ("Result:\n3\n", 0));
    let quiet = velme(&dir, &["check", ADD, "-q"]);
    assert_eq!((quiet.stdout.as_str(), quiet.stderr.as_str(), quiet.code), ("", "", 0));
    fs::write(
        dir.join("bad.velme"),
        "language: velme/0.1\n\ngoal Show(p: Playr) -> Number:\n    plan: \"x\"\n",
    )
    .expect("source");
    let quiet = velme(&dir, &["check", "bad.velme", "-q"]);
    assert_eq!(quiet.code, 1);
    assert!(quiet.stderr.contains("[VL0201]"), "{}", quiet.stderr);
}

/// `--color always` colours both streams and `never` neither, `NO_COLOR` set to any value turns `auto` off, and `auto` decides
/// for each stream by its own state (AC-CLI-25, R-CLI-28).
#[test]
fn ac_cli_25_color_is_decided_for_each_stream() {
    let dir = copy("add", "ac_cli_25");
    fs::write(
        dir.join("bad.velme"),
        "language: velme/0.1\n\ngoal Show(p: Playr) -> Number:\n    plan: \"x\"\n",
    )
    .expect("source");
    let always = velme(&dir, &["check", "bad.velme", "--color", "always"]);
    assert!(always.stdout.contains('\u{1b}') || always.stdout.is_empty());
    assert!(always.stderr.contains("\u{1b}["), "{}", always.stderr);
    let good = velme(&dir, &["check", ADD, "--color", "always"]);
    assert!(good.stdout.contains("\u{1b}[32m✓\u{1b}[0m Parsed"), "{}", good.stdout);
    let never = velme(&dir, &["check", "bad.velme", "--color", "never"]);
    assert!(!never.stderr.contains('\u{1b}') && !never.stdout.contains('\u{1b}'));
    // Neither stream is a terminal here, so `auto` colours neither, with or without NO_COLOR.
    let auto = velme(&dir, &["check", ADD]);
    assert!(!auto.stdout.contains('\u{1b}') && !auto.stderr.contains('\u{1b}'));
    let no_color = velme_env(&dir, &["check", ADD, "--color", "auto"], &[("NO_COLOR", "")], b"");
    assert!(!no_color.stdout.contains('\u{1b}'));
}

// ---- gc and cache clean ----

/// `velme gc` from a subdirectory deletes exactly the unreferenced artifact files and the leftover temporary files, and
/// prints the count; with no lock it refuses; `velme cache clean` exits 0 with no cache directory (AC-CLI-21, R-CLI-23,
/// R-CLI-24, D-108).
#[test]
fn ac_cli_21_gc_removes_unreferenced_files_and_cache_clean_needs_no_cache() {
    let dir = copy("double_then_add_one", "ac_cli_21");
    fs::write(dir.join("velme.toml"), "").expect("velme.toml");
    let referenced: Vec<String> = Lock::read(&dir)
        .expect("lock")
        .expect("a lock")
        .entries()
        .iter()
        .map(|e| e.artifact.hex())
        .collect();
    let store = dir.join(".velme/artifacts");
    let stray = format!("b3-{}.json", "0".repeat(64));
    fs::write(store.join(&stray), b"{}").expect("a stray artifact");
    let odd = store.join("notes.txt");
    fs::write(&odd, b"keep me").expect("a file that is not an artifact");
    fs::create_dir_all(dir.join(".velme/tmp")).expect("tmp");
    fs::write(dir.join(".velme/tmp/left-over"), b"x").expect("a temporary file");
    fs::create_dir_all(dir.join("sub/deeper")).expect("subdirectory");
    // Files touched in the last 10 minutes are a running build's, and stay (D-111).
    let recent_stray = format!("b3-{}.json", "1".repeat(64));
    fs::write(store.join(&recent_stray), b"{}").expect("a recent stray artifact");
    fs::write(dir.join(".velme/tmp/recent"), b"x").expect("a recent temporary file");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for path in [store.join(&stray), dir.join(".velme/tmp/left-over")] {
        fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(old))
            .expect("an old file");
    }
    let before = tree(&dir);

    let run = velme(&dir.join("sub/deeper"), &["gc"]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        ("Removed 2 unused files.\n", "", 0)
    );
    let mut expected = before.clone();
    expected.remove(&format!(".velme/artifacts/{stray}"));
    expected.remove(".velme/tmp/left-over");
    assert_eq!(
        tree(&dir),
        expected,
        "exactly the old stray artifact and the old temporary file"
    );
    for hex in &referenced {
        assert!(store.join(format!("b3-{hex}.json")).is_file());
    }
    assert!(odd.is_file() && dir.join("velme.lock").is_file());
    let run = velme(&dir.join("sub"), &["gc", "--json"]);
    let envelope = parsed(&run);
    assert_eq!(envelope["summary"]["removed"], 0);
    assert!(store.join(&recent_stray).is_file() && dir.join(".velme/tmp/recent").is_file());
    let run = velme(&dir.join("sub"), &["gc"]);
    assert_eq!(run.stdout, "Removed 0 unused files.\n");

    // No lock: refused, and nothing is deleted.
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    fs::write(store.join(&stray), b"{}").expect("a stray artifact");
    let run = velme(&dir.join("sub"), &["gc"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(
        run.stderr.contains("[VL0901]") && run.stderr.contains("velme.lock"),
        "{}",
        run.stderr
    );
    assert!(store.join(&stray).is_file());

    // A directory with no project at all: the current directory is the root, and it has no lock.
    let empty = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli")
        .join("ac_cli_21_empty");
    let _ = fs::remove_dir_all(&empty);
    let _ = fs::remove_dir_all(user_cache(&empty));
    fs::create_dir_all(&empty).expect("directory");
    assert_eq!(velme(&empty, &["gc"]).code, 64);

    // `cache clean` with no cache directory is done and exits 0; with one it removes it.
    let run = velme(&empty, &["cache", "clean"]);
    assert_eq!(
        (run.stdout.as_str(), run.code),
        ("There is no WASM module cache to remove.\n", 0)
    );
    let envelope = parsed(&velme(&empty, &["cache", "clean", "--json"]));
    assert_eq!(
        (envelope["status"].as_str(), envelope["summary"]["removed"].as_u64()),
        (Some("ok"), Some(0))
    );
    let cache = user_cache(&empty).join("velme/wasm");
    fs::create_dir_all(&cache).expect("cache");
    fs::write(cache.join("m.cwasm"), b"x").expect("module");
    let envelope = parsed(&velme(&empty, &["cache", "clean", "--json"]));
    assert_eq!(envelope["summary"]["removed"], 1, "a directory removed counts as 1");
    fs::create_dir_all(&cache).expect("cache");
    fs::write(cache.join("m.cwasm"), b"x").expect("module");
    let run = velme(&empty, &["cache", "clean"]);
    assert_eq!((run.stdout.as_str(), run.code), ("Removed the WASM module cache.\n", 0));
    assert!(!cache.exists());
    assert!(
        user_cache(&empty).join("velme").exists(),
        "only the wasm directory goes"
    );
}

/// A diagnostic with no place in a source file carries the file it is about in `file`, and the span `0,0,1,1` for "no place":
/// `velme.toml`, the user-level config, `velme.lock`, and the empty string for a bad flag (D-111, R-CLI-15).
#[test]
fn ac_cli_12_a_diagnostic_with_no_place_names_the_file_it_is_about() {
    let dir = copy("add", "ac_cli_12_no_place");
    let no_place = |run: &Run, file: &str| {
        let envelope = parsed(run);
        let d = &envelope["diagnostics"][0];
        assert_eq!(d["file"], file, "{envelope}");
        assert_eq!(
            d["span"],
            serde_json::json!({"start": 0, "end": 0, "line": 1, "column": 1}),
            "{envelope}"
        );
    };
    write_project_config(&dir, "[synthesis\n");
    no_place(&velme(&dir, &["check", ADD, "--json"]), "velme.toml");
    write_project_config(&dir, "");
    let user = write_user_config(&dir, "[synthesis]\nmodel = \"x\"\n");
    let run = velme(&dir, &["check", ADD, "--json"]);
    let shown = parsed(&run)["diagnostics"][0]["file"]
        .as_str()
        .expect("a file")
        .to_owned();
    assert!(shown.ends_with("config.toml"), "{shown}");
    assert_eq!(shown, user.to_string_lossy().replace('\\', "/"));
    no_place(&run, &shown);
    fs::remove_file(&user).expect("user config removed");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let run = velme(&dir, &["gc", "--json"]);
    no_place(&run, "velme.lock");
    // A bad command line is about no file, even with one on it.
    no_place(&velme(&dir, &["check", ADD, "--frobnicate", "--json"]), "");
}

/// Human output names the file a placeless diagnostic is about, and has no empty header when there is none; a usage error
/// honours `--color always` (D-111, R-CLI-28).
#[test]
fn ac_cli_12_human_output_names_the_file_a_diagnostic_is_about() {
    let dir = copy("add", "ac_cli_12_human");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let run = velme(&dir, &["gc"]);
    assert!(
        run.stderr.contains("[VL0901]") && run.stderr.contains("╭─[ velme.lock ]"),
        "{}",
        run.stderr
    );
    write_project_config(&dir, "[synthesis\n");
    let run = velme(&dir, &["check", ADD]);
    assert!(
        run.stderr.contains("╭─[ velme.toml ]") && !run.stderr.contains(ADD),
        "{}",
        run.stderr
    );
    write_project_config(&dir, "");
    let run = velme(&dir, &["check", ADD, "--input", "-", "--frobnicate"]);
    assert!(!run.stderr.contains("╭─[  ]"), "{}", run.stderr);
    let run = velme(&dir, &["check", ADD, "--color", "always", "--frobnicate"]);
    assert!(
        run.stderr.contains('\u{1b}'),
        "a usage error is coloured: {}",
        run.stderr
    );
    let run = velme(&dir, &["check", ADD, "--frobnicate"]);
    assert!(!run.stderr.contains('\u{1b}'), "{}", run.stderr);
    // An unreadable `--input` file is about that file.
    let run = velme(&dir, &["run", ADD, "--goal", "Add", "--input", "nope.json", "--json"]);
    assert_eq!(parsed(&run)["diagnostics"][0]["file"], "nope.json", "{}", run.stdout);
}

/// `gc` and `cache clean` never delete through a link (R-CLI-23, R-CLI-24).
#[cfg(unix)]
#[test]
fn gc_and_cache_clean_never_delete_through_a_link() {
    let dir = copy("add", "gc_links");
    fs::write(dir.join("velme.toml"), "").expect("velme.toml");
    let outside = dir.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(outside.join("precious"), b"x").expect("file");
    // A link named like an artifact stays; so does the file it points at.
    let link = dir.join(format!(".velme/artifacts/b3-{}.json", "1".repeat(64)));
    std::os::unix::fs::symlink(outside.join("precious"), &link).expect("link");
    let run = velme(&dir, &["gc"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(link.symlink_metadata().is_ok() && outside.join("precious").is_file());
    // A `.velme` that is a link is refused.
    let project = copy("add", "gc_links_velme");
    fs::write(project.join("velme.toml"), "").expect("velme.toml");
    fs::rename(project.join(".velme"), outside.join("store")).expect("moved");
    std::os::unix::fs::symlink(outside.join("store"), project.join(".velme")).expect("link");
    let run = velme(&project, &["gc"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0901]"), "{}", run.stderr);
    // A cache directory that is a link is left alone.
    let home = user_cache(&dir).join("velme");
    fs::create_dir_all(&home).expect("cache parent");
    std::os::unix::fs::symlink(&outside, home.join("wasm")).expect("link");
    let run = velme(&dir, &["cache", "clean"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    assert!(outside.join("precious").is_file());
}

// ---- artifacts on an older compatibility unit (AC-BLT-10) ----

/// A lock hand-written for an artifact built on an older builtins compatibility unit is stale: `velme run --locked` exits 4
/// with `VL0702`, and never runs the artifact (AC-BLT-10, D-85).
#[test]
fn ac_blt_10_an_artifact_built_on_an_older_compatibility_unit_is_stale() {
    let dir = copy("add", "ac_blt_10");
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let entry = lock.entries().first().expect("an entry").clone();
    // The artifact as an older release wrote it: the same document with builtins version `0.0`, stored under its own hash.
    let store = Store::new(&dir);
    let path = store.path(entry.artifact);
    let old_key = velme_ir::Fingerprint::of_bytes(b"contract of builtins 0.0");
    let old = fs::read_to_string(&path)
        .expect("artifact")
        .replace("\"builtins_version\":\"0.1\"", "\"builtins_version\":\"0.0\"")
        .replace(
            &format!("\"contract_key\":\"{}\"", entry.contract_key),
            &format!("\"contract_key\":\"{old_key}\""),
        );
    assert!(
        old.contains("\"builtins_version\":\"0.0\""),
        "the manifest and the IR both say so"
    );
    let id = velme_ir::Fingerprint::of_bytes(old.as_bytes());
    fs::write(store.path(id), &old).expect("older artifact");
    // The lock as that release wrote it: its `contract_key` is that of the older unit.
    let mut written = Lock::new(lock.language.clone());
    written.insert(Entry {
        contract_key: old_key,
        artifact: id,
        ..entry
    });
    written.write(&dir).expect("lock written");
    let before = tree(&dir);
    let run = velme(
        &dir,
        &[
            "run", ADD, "--goal", "Add", "--arg", "a=1", "--arg", "b=2", "--locked", "--json",
        ],
    );
    assert_eq!(run.code, 4, "{}", run.stderr);
    let envelope = parsed(&run);
    assert_eq!(codes(&envelope), ["VL0702"]);
    let message = envelope["results"][0]["diagnostics"][0].to_string();
    assert!(
        message.contains("0.0") && message.contains("0.1"),
        "the versions are named: {message}"
    );
    assert!(envelope["results"][0].get("result").is_none());
    assert_eq!(tree(&dir), before, "nothing is written");
}
