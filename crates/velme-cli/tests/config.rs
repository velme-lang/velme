//! Where a build's settings come from (`tooling/40` §5, R-CLI-11, R-CLI-13, R-CLI-26, D-105, D-110), through the binary on the
//! scripted provider and on an Ollama mock server. Needs the `test-provider` feature, which the gate turns on (D-94).
#![cfg(feature = "test-provider")]
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::schema::assert_cli_envelope;

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

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-config").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    fs::write(dir.join("game.velme"), SOURCE).expect("source");
    dir
}

/// The body `n <op> k` as a scripted reply (D-103).
fn reply(op: &str, k: i64) -> Value {
    let number = json!({"t": "Number"});
    json!({"body": {"kind": "binary", "op": op, "left": {"kind": "input", "name": "n"},
                    "right": {"kind": "literal", "type": number, "value": k}}})
}

fn script(dir: &Path, replies: &[Value]) -> PathBuf {
    let path = dir.join("script.json");
    let texts: Vec<String> = replies.iter().map(Value::to_string).collect();
    fs::write(&path, serde_json::to_string(&texts).expect("json")).expect("script");
    path
}

fn user_config(dir: &Path, text: &str) {
    let file = dir.join("home").join("velme").join("config.toml");
    fs::create_dir_all(file.parent().expect("a parent")).expect("directory");
    fs::write(file, text).expect("user config");
}

/// `velme` in `dir` with the user-level config in `dir/home` and `envs` set.
fn velme(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Out {
    let home = dir.join("home");
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
        .env_remove("VELME_OLLAMA_URL")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .envs(envs.iter().copied())
        .output()
        .expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn files_text(dir: &Path, found: &mut String) {
    for entry in fs::read_dir(dir).expect("directory").filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files_text(&path, found);
        } else if let Ok(text) = fs::read_to_string(&path) {
            found.push_str(&text);
        }
    }
}

/// An API key in the environment appears in no output and no file of a build, a run, a trace, a test or an artifact listing,
/// in either mode (AC-CLI-08, R-SEC-06, R-CLI-12).
#[test]
fn ac_cli_08_an_api_key_never_appears_in_any_output_or_file() {
    let dir = project("ac_cli_08");
    let script = script(&dir, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let envs = [
        ("VELME_API_KEY", "sk-SENTINEL-ONE-0123456789"),
        ("ANTHROPIC_API_KEY", "sk-SENTINEL-TWO-0123456789"),
        ("VELME_SYNTH_SCRIPT", script.as_str()),
    ];
    let mut everything = String::new();
    for args in [
        &["build", "game.velme", "--provider", "scripted"][..],
        &["build", "game.velme", "--provider", "scripted", "-v", "--json"],
        &["run", "game.velme", "--goal", "Both", "--arg", "n=4", "--json"],
        &["trace", "game.velme", "--goal", "Both", "--arg", "n=4", "--json"],
        &["trace", "game.velme", "--goal", "Both", "--arg", "n=4"],
        &["test", "game.velme", "--json"],
        &["artifact", "game.velme", "--goal", "Double", "--json"],
        &["check", "game.velme"],
        &["explain", "game.velme", "--goal", "Both"],
        &[
            "build",
            "game.velme",
            "--provider",
            "anthropic",
            "--model",
            "m",
            "--offline",
        ],
    ] {
        let out = velme(&dir, args, &envs);
        assert!(out.code == 0 || out.code == 4, "{args:?}: {}{}", out.stdout, out.stderr);
        everything.push_str(&out.stdout);
        everything.push_str(&out.stderr);
    }
    files_text(&dir.join(".velme"), &mut everything);
    files_text(&dir, &mut everything);
    assert!(!everything.contains("SENTINEL"), "a key reached an output or a file");
}

/// A user-level `max_calls_per_build` below the project's makes the build stop at the ceiling, and the notice names both
/// values (AC-CLI-17, R-SEC-12, D-50).
#[test]
fn ac_cli_17_a_user_ceiling_stops_the_build_and_the_notice_names_both_values() {
    let dir = project("ac_cli_17");
    fs::write(dir.join("velme.toml"), "[synthesis]\nmax_calls_per_build = 3\n").expect("velme.toml");
    user_config(&dir, "[synthesis]\nmax_calls_per_build = 1\n");
    let script = script(&dir, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Double  ✓ built"), "{}", out.stdout);
    assert!(
        out.stderr.contains("reached its limit of 1 provider calls"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains("The project asked for max_calls_per_build = 3; your ceiling of 1 applies."),
        "{}",
        out.stderr
    );
    // The same in the envelope's notices.
    let _ = fs::remove_dir_all(dir.join(".velme"));
    let _ = fs::remove_file(dir.join("velme.lock"));
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted", "--json"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_cli_envelope(&envelope);
    let notice = envelope["notices"][0].as_str().expect("a notice");
    assert!(notice.contains("= 3") && notice.contains("ceiling of 1"), "{notice}");
    // With no ceiling, the project's own three calls are enough.
    user_config(&dir, "");
    let _ = fs::remove_dir_all(dir.join(".velme"));
    let _ = fs::remove_file(dir.join("velme.lock"));
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(!out.stderr.contains("ceiling"), "{}", out.stderr);
}

/// A model outside `allowed_models` — from the flag, `VELME_MODEL`, the project, or as `retry_model` — is `VL0405`, before
/// any contact, and is not swapped for an allowed one (AC-CLI-23, R-CLI-26, D-105).
#[test]
fn ac_cli_23_a_model_outside_allowed_models_is_vl0405_and_never_swapped() {
    let dir = project("ac_cli_23");
    user_config(&dir, "[synthesis]\nallowed_models = [\"good-model\"]\n");
    let key = [("VELME_API_KEY", "sk-test-key-0123456789")];
    /// Flags, environment, the project's `velme.toml`, and the model the error must name.
    type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)], &'a str, &'a str);
    let cases: [Case; 4] = [
        (
            &["--provider", "anthropic", "--model", "bad-model"],
            &key,
            "",
            "bad-model",
        ),
        (
            &["--provider", "anthropic"],
            &[key[0], ("VELME_MODEL", "bad-model")],
            "",
            "bad-model",
        ),
        (
            &["--provider", "anthropic"],
            &key,
            "[synthesis]\nmodel = \"bad-model\"\n",
            "bad-model",
        ),
        (
            &["--provider", "anthropic", "--model", "good-model"],
            &key,
            "[synthesis]\nretry_model = \"worse-model\"\n",
            "worse-model",
        ),
    ];
    for (i, (flags, envs, toml, wrong)) in cases.into_iter().enumerate() {
        fs::write(dir.join("velme.toml"), toml).expect("velme.toml");
        let mut args = vec!["build", "game.velme", "--json"];
        args.extend_from_slice(flags);
        let out = velme(&dir, &args, envs);
        assert_eq!(out.code, 2, "case {i}: {}{}", out.stdout, out.stderr);
        assert_eq!(out.stderr, "", "case {i}: nothing was contacted, so there is no notice");
        let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
        assert_cli_envelope(&envelope);
        let first = &envelope["results"][0]["diagnostics"][0];
        assert_eq!(first["code"], "VL0405", "case {i}: {envelope}");
        assert_eq!(
            first["message"],
            format!("The model `{wrong}` isn't in your allowed models."),
            "case {i}"
        );
        assert!(
            first["help"].as_str().expect("help").contains("`good-model`"),
            "case {i}"
        );
        assert!(
            !dir.join("velme.lock").exists(),
            "case {i}: nothing was built with another model"
        );
    }
    // Ollama is held to the list too.
    fs::write(dir.join("velme.toml"), "").expect("velme.toml");
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "ollama", "--model", "llama3"],
        &[],
    );
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("isn't in your allowed models"), "{}", out.stderr);
}

/// A project's `[synthesis] provider` chooses the provider when no flag does, and the flag wins over it (R-CLI-11).
#[test]
fn a_project_names_its_provider_and_the_flag_wins() {
    let dir = project("provider_choice");
    // `scripted` is flag-only: a project naming it is VL0902 (D-111).
    fs::write(dir.join("velme.toml"), "[synthesis]\nprovider = \"scripted\"\n").expect("velme.toml");
    let out = velme(&dir, &["build", "game.velme"], &[]);
    assert_eq!(out.code, 64, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("VL0902") && out.stderr.contains("anthropic or ollama or external or replay"),
        "{}",
        out.stderr
    );
    fs::write(dir.join("velme.toml"), "[synthesis]\nprovider = \"nope\"\n").expect("velme.toml");
    let out = velme(&dir, &["build", "game.velme"], &[]);
    assert_eq!(out.code, 64, "{}{}", out.stdout, out.stderr);
    let fresh = project("provider_choice_flag");
    fs::write(fresh.join("velme.toml"), "[synthesis]\nprovider = \"anthropic\"\n").expect("velme.toml");
    let none = velme(&fresh, &["build", "game.velme"], &[]);
    assert_eq!(
        none.code, 2,
        "the project's anthropic has no key: {}{}",
        none.stdout, none.stderr
    );
    let script = script(&fresh, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let out = velme(
        &fresh,
        &["build", "game.velme", "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    assert_eq!(
        out.code, 0,
        "the flag wins over the project: {}{}",
        out.stdout, out.stderr
    );
}

/// `--build` on `run`, `test` and `trace` builds stale goals first and then does its own work; without it nothing
/// synthesizes (R-CLI-03, D-28).
#[test]
fn build_on_run_test_and_trace_builds_first() {
    let dir = project("build_flag");
    let script = script(&dir, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let envs = [("VELME_SYNTH_SCRIPT", script.as_str())];
    let run = velme(&dir, &["run", "game.velme", "--goal", "Both", "--arg", "n=4"], &[]);
    assert_eq!(
        run.code, 4,
        "without --build the goals are simply not built: {}",
        run.stderr
    );
    let run = velme(
        &dir,
        &[
            "run",
            "game.velme",
            "--goal",
            "Both",
            "--arg",
            "n=4",
            "--build",
            "--provider",
            "scripted",
        ],
        &envs,
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains("Double  ✓ built") && run.stdout.contains("Result:\n9\n"),
        "{}",
        run.stdout
    );
    assert!(run.stderr.contains("Nothing is sent"), "{}", run.stderr);
    // Built already: `--build` makes no call, and the script isn't needed.
    let run = velme(
        &dir,
        &["test", "game.velme", "--build", "--provider", "scripted", "--json"],
        &envs,
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let envelope: Value = serde_json::from_str(&run.stdout).expect("JSON");
    assert_cli_envelope(&envelope);
    assert_eq!(envelope["summary"]["calls"], 0);
    assert_eq!(envelope["results"].as_array().map(Vec::len), Some(3));
    let run = velme(
        &dir,
        &[
            "trace",
            "game.velme",
            "--goal",
            "Both",
            "--arg",
            "n=1",
            "--build",
            "--provider",
            "scripted",
        ],
        &envs,
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    // A build that fails stops the command before it runs anything.
    let dir = project("build_flag_fails");
    let run = velme(
        &dir,
        &["run", "game.velme", "--goal", "Both", "--arg", "n=4", "--build"],
        &[],
    );
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("[VL0405]") && !run.stdout.contains("Result"),
        "{}{}",
        run.stdout,
        run.stderr
    );
}

/// The Ollama server's URL comes from `--ollama-url`, `VELME_OLLAMA_URL` or the user-level config, in that order, and the
/// request carries the provider's default output limit, the project's, or the user's lower ceiling (R-CLI-13, D-105,
/// D-110).
#[test]
fn the_ollama_url_and_output_limit_come_from_flag_env_config_and_ceiling() {
    let serve = || {
        MockServer::start([
            MockResponse::ollama_tags(&[("m:latest", DIGEST)]),
            MockResponse::ollama_chat(&reply("mul", 2)),
        ])
    };
    let chat = |server: &MockServer| {
        server
            .requests()
            .iter()
            .find(|r| r.path == "/api/chat")
            .map(velme_test_support::mock::MockRequest::json)
    };
    let one_goal = |name: &str| {
        let dir = project(name);
        fs::write(
            dir.join("game.velme"),
            "language: velme/0.1\n\ngoal Double(n: Number) -> Number:\n    plan: \"Double it.\"\n    examples:\n        - Double(2) == 4\n",
        )
        .expect("source");
        dir
    };
    let args = ["build", "game.velme", "--provider", "ollama", "--model", "m"];

    // Flag first.
    let (server, dead) = (serve(), serve());
    let dir = one_goal("ollama_flag");
    user_config(&dir, &format!("[synthesis]\nollama_url = \"{}\"\n", dead.url()));
    let mut with_flag = args.to_vec();
    with_flag.extend(["--ollama-url", server.url()]);
    let out = velme(&dir, &with_flag, &[("VELME_OLLAMA_URL", dead.url())]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains(&format!(
            "the Ollama server at {}, model m,",
            server
                .url()
                .trim_start_matches("http://")
                .split(':')
                .next()
                .unwrap_or_default()
        )) && !out.stderr.contains(server.url()),
        "the notice names the host only: {}",
        out.stderr
    );
    assert_eq!(chat(&server).expect("the chat")["options"]["num_predict"], 2048);
    assert!(dead.requests().is_empty(), "the flag wins");

    // Then the environment, over the user-level config.
    let (server, dead) = (serve(), serve());
    let dir = one_goal("ollama_env");
    user_config(&dir, &format!("[synthesis]\nollama_url = \"{}\"\n", dead.url()));
    let out = velme(&dir, &args, &[("VELME_OLLAMA_URL", server.url())]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(chat(&server).is_some() && dead.requests().is_empty());

    // Then the user-level config, with the project's output limit clamped by the user's ceiling.
    let server = serve();
    let dir = one_goal("ollama_config");
    fs::write(dir.join("velme.toml"), "[synthesis]\nmax_output_tokens = 500\n").expect("velme.toml");
    user_config(
        &dir,
        &format!(
            "[synthesis]\nollama_url = \"{}\"\nmax_output_tokens = 100\n",
            server.url()
        ),
    );
    let out = velme(&dir, &args, &[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(chat(&server).expect("the chat")["options"]["num_predict"], 100);
    assert!(
        out.stderr
            .contains("The project asked for max_output_tokens = 500; your ceiling of 100 applies."),
        "{}",
        out.stderr
    );

    // The project's own limit, when no ceiling is lower.
    let server = serve();
    let dir = one_goal("ollama_project_limit");
    fs::write(dir.join("velme.toml"), "[synthesis]\nmax_output_tokens = 500\n").expect("velme.toml");
    let out = velme(&dir, &args, &[("VELME_OLLAMA_URL", server.url())]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(chat(&server).expect("the chat")["options"]["num_predict"], 500);

    // A URL that isn't https or plain http to this machine is VL0902 before any contact, from every source.
    let dir = one_goal("ollama_bad_url");
    for (flag, env) in [(Some("http://example.com"), None), (None, Some("ftp://localhost"))] {
        let mut args = args.to_vec();
        args.extend(flag.into_iter().flat_map(|f| ["--ollama-url", f]));
        let envs: Vec<(&str, &str)> = env.into_iter().map(|e| ("VELME_OLLAMA_URL", e)).collect();
        let out = velme(&dir, &args, &envs);
        assert_eq!(out.code, 64, "{}{}", out.stdout, out.stderr);
        assert!(
            out.stderr.contains("[VL0902]") && out.stderr.contains("Ollama URL"),
            "{}",
            out.stderr
        );
    }
    user_config(&dir, "[synthesis]\nollama_url = \"http://example.com\"\n");
    let out = velme(&dir, &args, &[]);
    assert_eq!(out.code, 64, "{}{}", out.stdout, out.stderr);
}

/// A ceiling on a value the project never set says the default was cut, not that the project asked for it (D-105).
#[test]
fn a_ceiling_over_a_default_says_the_default_is_cut() {
    let dir = project("ceiling_default");
    user_config(&dir, "[synthesis]\nmax_calls_per_build = 1\n");
    let script = script(&dir, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    assert!(
        out.stderr
            .contains("The default is max_calls_per_build = 50; your ceiling of 1 applies."),
        "{}",
        out.stderr
    );
}

/// Bad config is reported and wins the exit code (64) even when the source has an error too, on every command that reads a
/// file (R-CLI-16, R-CLI-25).
#[test]
fn bad_config_is_reported_beside_a_source_error_on_every_command() {
    let dir = project("bad_config_and_source");
    fs::write(
        dir.join("game.velme"),
        "language: velme/0.1\n\ngoal Broken(n: Number -> Number:\n",
    )
    .expect("source");
    fs::write(dir.join("velme.toml"), "[synthesis]\nmax_retries = 9\n").expect("velme.toml");
    for args in [
        &["check", "game.velme"][..],
        &["build", "game.velme"],
        &["run", "game.velme", "--goal", "Broken"],
        &["test", "game.velme"],
        &["trace", "game.velme", "--goal", "Broken"],
        &["explain", "game.velme", "--goal", "Broken"],
        &["artifact", "game.velme", "--goal", "Broken"],
    ] {
        let mut args = args.to_vec();
        args.push("--json");
        let out = velme(&dir, &args, &[]);
        assert_eq!(out.code, 64, "{args:?}: {}{}", out.stdout, out.stderr);
        let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
        let found: Vec<&str> = envelope["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .filter_map(|d| d["code"].as_str())
            .collect();
        assert!(found.contains(&"VL0902"), "{args:?}: {found:?}");
        assert!(
            found.iter().any(|c| *c != "VL0902"),
            "{args:?}: the source error too: {found:?}"
        );
    }
}

/// `[budget]` lowers the limits of a goal without a `budget` line and is part of its `contract_key`, so a locked goal built
/// without it is stale with it; values above the system caps change nothing; a bad quantity or `[artifacts] dir` is VL0902
/// (R-ART-23, D-8, D-81).
#[test]
fn a_project_budget_reaches_the_contract_key_and_bad_values_are_vl0902() {
    let dir = project("budget");
    let script = script(&dir, &[reply("mul", 2), reply("add", 1)]);
    let script = script.to_string_lossy().into_owned();
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", script.as_str())],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let run = |dir: &Path| {
        velme(
            dir,
            &[
                "run",
                "game.velme",
                "--goal",
                "Both",
                "--arg",
                "n=4",
                "--locked",
                "--json",
            ],
            &[],
        )
    };
    assert_eq!(run(&dir).code, 0);
    // Above every cap: the same limits, so the same key.
    fs::write(
        dir.join("velme.toml"),
        "[budget]\ncpu = \"99999999ms\"\nmemory = \"99999mb\"\ncalls = 99999999\ndepth = 99999999\n",
    )
    .expect("velme.toml");
    assert_eq!(run(&dir).code, 0, "a budget above the caps is the caps");
    // Below one: a different key.
    fs::write(dir.join("velme.toml"), "[budget]\ncpu = \"50ms\"\n").expect("velme.toml");
    let stale = run(&dir);
    assert_eq!(stale.code, 4, "{}{}", stale.stdout, stale.stderr);
    assert!(stale.stdout.contains("VL0702"), "{}", stale.stdout);
    for (text, expected) in [
        (
            "[budget]\nmemory = \"99999999999999999mb\"\n",
            "`budget.memory` in `velme.toml` should be",
        ),
        (
            "[artifacts]\ndir = \".velme/artifacts\"\n",
            "`artifacts.dir` in `velme.toml` isn't available yet.",
        ),
        (
            "[synthesis]\nprovider = \"gpt\"\n",
            "`synthesis.provider` in `velme.toml` should be one of",
        ),
    ] {
        fs::write(dir.join("velme.toml"), text).expect("velme.toml");
        let out = velme(&dir, &["check", "game.velme", "--json"], &[]);
        assert_eq!(out.code, 64, "{text}: {}{}", out.stdout, out.stderr);
        assert!(out.stdout.contains(expected), "{text}: {}", out.stdout);
    }
}

/// An empty flag or variable doesn't hide the next source of the Ollama URL, and `retry_model` is asked for on the retries,
/// with both models and their digests in the identity (R-CLI-13, R-SYNTH-39).
#[test]
fn an_empty_url_source_is_skipped_and_ollama_retries_with_its_retry_model() {
    let server = MockServer::start([
        MockResponse::ollama_tags(&[("m:latest", DIGEST), ("r:latest", DIGEST)]),
        MockResponse::ollama_chat(&reply("mul", 3)),
        MockResponse::ollama_chat(&reply("mul", 2)),
    ]);
    let dir = project("ollama_retry");
    fs::write(
        dir.join("game.velme"),
        "language: velme/0.1\n\ngoal Double(n: Number) -> Number:\n    plan: \"Double it.\"\n    examples:\n        - Double(2) == 4\n",
    )
    .expect("source");
    fs::write(dir.join("velme.toml"), "[synthesis]\nretry_model = \"r\"\n").expect("velme.toml");
    user_config(&dir, &format!("[synthesis]\nollama_url = \"{}\"\n", server.url()));
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "ollama", "--model", "m"],
        &[("VELME_OLLAMA_URL", "")],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let models: Vec<String> = server
        .requests()
        .iter()
        .filter(|r| r.path == "/api/chat")
        .map(|r| r.json()["model"].as_str().expect("a model").to_owned())
        .collect();
    assert_eq!(models, ["m:latest", "r:latest"]);
    let lock = fs::read_to_string(dir.join("velme.lock")).expect("lock");
    assert!(lock.contains("Double"), "{lock}");
    let mut everything = String::new();
    files_text(&dir.join(".velme"), &mut everything);
    assert!(
        everything.contains(&format!("m:latest@{DIGEST}+r:latest@{DIGEST}")),
        "{everything}"
    );
}

/// A missing `retry_model` is named in the `VL0405`, not the primary (D-111); `allowed_models` matches an Ollama model with or
/// without `:latest` on either side.
#[test]
fn a_missing_ollama_retry_model_is_named_and_allowed_models_ignore_latest() {
    let server = MockServer::start([MockResponse::ollama_tags(&[("m:latest", DIGEST)])]);
    let dir = project("ollama_missing_retry");
    fs::write(dir.join("velme.toml"), "[synthesis]\nretry_model = \"r\"\n").expect("velme.toml");
    user_config(
        &dir,
        &format!(
            "[synthesis]\nollama_url = \"{}\"\nallowed_models = [\"m:latest\", \"r\"]\n",
            server.url()
        ),
    );
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "ollama", "--model", "m", "--json"],
        &[],
    );
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_cli_envelope(&envelope);
    let first = &envelope["results"][0]["diagnostics"][0];
    assert_eq!(first["code"], "VL0405", "{envelope}");
    assert_eq!(first["message"], "The Ollama server doesn't have the model `r`.");
    assert!(
        first["help"].as_str().expect("help").contains("ollama pull r"),
        "{envelope}"
    );
    // `m` was allowed as `m:latest`, `r` as itself: neither was refused by the list.
    assert!(!out.stdout.contains("isn't in your allowed models"), "{}", out.stdout);
}

/// `run --build` decodes its arguments first, so a typo costs no build and no provider call (R-CLI-03).
#[test]
fn run_build_decodes_its_arguments_before_building() {
    let dir = project("build_bad_arg");
    let out = velme(
        &dir,
        &[
            "run",
            "game.velme",
            "--goal",
            "Both",
            "--arg",
            "zz=4",
            "--build",
            "--provider",
            "ollama",
            "--model",
            "m",
        ],
        &[],
    );
    assert_eq!(out.code, 64, "{}{}", out.stdout, out.stderr);
    assert!(!dir.join("velme.lock").exists());
    assert!(
        !out.stderr.contains("Sending"),
        "no provider was contacted: {}",
        out.stderr
    );
}
