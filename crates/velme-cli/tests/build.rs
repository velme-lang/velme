//! `velme build` on the scripted provider (`tooling/40` §2, `compiler/22` R-SYNTH-05): the whole flow from identity to
//! lock, through the binary. Needs the `test-provider` feature, which the gate turns on (D-94).
#![cfg(feature = "test-provider")]
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use velme_test_support::velme_command;

use serde_json::{Value, json};

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

/// A reply holding the body `n <op> k` (D-103).
fn reply(op: &str, k: i64) -> String {
    let number = json!({"t": "Number"});
    json!({"body": {"kind": "binary", "op": op, "left": {"kind": "input", "name": "n"},
                    "right": {"kind": "literal", "type": number, "value": k}}})
    .to_string()
}

fn double() -> String {
    reply("mul", 2)
}

fn add_one() -> String {
    reply("add", 1)
}

/// Writes a script of replies, as JSON strings (`tooling/40` §5.2).
fn script(dir: &Path, replies: &[String]) -> PathBuf {
    let path = dir.join("script.json");
    fs::write(&path, serde_json::to_string(replies).expect("json")).expect("script");
    path
}

fn velme(dir: &Path, args: &[&str], script: Option<&Path>) -> Out {
    velme_with(dir, args, script, &[])
}

/// `velme` with `envs` set. The user's own provider settings are removed first, so no test can reach a real provider.
fn velme_with(dir: &Path, args: &[&str], script: Option<&Path>, envs: &[(&str, &str)]) -> Out {
    let mut command = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_SCRIPT")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .envs(envs.iter().copied());
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

/// `--provider anthropic` without a key is `VL0405` naming the variable to set, with no notice and nothing written;
/// with a key but no model it asks for the model. A sentinel key appears in no output stream, in either mode, and in
/// no file the build wrote (AC-SEC-05, R-CLI-12, R-SEC-06).
#[test]
fn ac_sec_05_a_missing_key_or_model_is_vl0405_and_the_key_is_never_printed() {
    const SENTINEL: &str = "sk-ant-SENTINEL-KEY-0123456789";
    let dir = project("anthropic-config");
    let no_key = velme(&dir, &["build", "game.velme", "--provider", "anthropic"], None);
    assert_eq!(no_key.code, 2, "{}{}", no_key.stdout, no_key.stderr);
    assert!(
        no_key.stderr.contains("[VL0405]") && no_key.stderr.contains("VELME_API_KEY"),
        "{}",
        no_key.stderr
    );
    assert!(
        !no_key.stderr.contains("Sending your plans"),
        "nothing was contacted: {}",
        no_key.stderr
    );
    for extra in [&[][..], &["--json"][..]] {
        let mut args = vec!["build", "game.velme", "--provider", "anthropic"];
        args.extend_from_slice(extra);
        let no_model = velme_with(&dir, &args, None, &[("VELME_API_KEY", SENTINEL)]);
        assert_eq!(no_model.code, 2, "{}{}", no_model.stdout, no_model.stderr);
        assert!(
            no_model.stderr.contains("[VL0405]") || no_model.stdout.contains("VL0405"),
            "{}{}",
            no_model.stdout,
            no_model.stderr
        );
        assert!(no_model.stdout.contains("VELME_MODEL") || no_model.stderr.contains("VELME_MODEL"));
        for text in [&no_model.stdout, &no_model.stderr] {
            assert!(!text.contains(SENTINEL) && !text.contains("SENTINEL"), "{text}");
        }
    }
    assert!(!dir.join("velme.lock").exists());
    // `--model` counts as the model, so with a key the build gets as far as needing a script it doesn't have: the
    // model complaint is gone (the provider isn't contacted: a cached-nothing build fails at the first request only
    // with a real key, so this stops at the flag check by using no key).
    let flag = velme(
        &dir,
        &["build", "game.velme", "--provider", "anthropic", "--model", "m"],
        None,
    );
    assert!(
        flag.stderr.contains("API key") && !flag.stderr.contains("needs a model"),
        "{}",
        flag.stderr
    );
}

/// `VELME_SYNTH_RECORD=1` records a build's exchanges into the project's fixture directory, and a `replay` build of the
/// same file from those fixtures writes the same lock and artifacts (AC-SYNTH-10, R-SYNTH-43, D-94).
#[test]
fn ac_synth_10_a_recorded_build_replays_to_the_same_lock_and_artifacts() {
    let recorded = project("recorded");
    let script = script(&recorded, &[double(), add_one()]);
    let first = velme_with(
        &recorded,
        &["build", "game.velme", "--provider", "scripted"],
        Some(&script),
        &[("VELME_SYNTH_RECORD", "1")],
    );
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let fixtures = recorded.join("tests/fixtures/synth");
    assert!(fixtures.join("replay.json").is_file());
    let names: Vec<String> = fs::read_dir(&fixtures)
        .expect("fixtures")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.iter().filter(|n| n.starts_with("b3-")).count(), 2, "{names:?}");
    for name in &names {
        let text = fs::read_to_string(fixtures.join(name)).expect("fixture");
        assert!(!text.contains("Double it."), "no plan text in {name}: {text}");
    }

    let replayed = project("replayed");
    let target = replayed.join("tests/fixtures/synth");
    fs::create_dir_all(&target).expect("fixture directory");
    for name in &names {
        fs::copy(fixtures.join(name), target.join(name)).expect("copied");
    }
    let second = velme(&replayed, &["build", "game.velme", "--provider", "replay"], None);
    assert_eq!(second.code, 0, "{}{}", second.stdout, second.stderr);
    assert_eq!(
        fs::read(replayed.join("velme.lock")).expect("lock"),
        fs::read(recorded.join("velme.lock")).expect("lock")
    );
    assert_eq!(artifacts(&replayed), artifacts(&recorded));
    for name in artifacts(&recorded) {
        assert_eq!(
            fs::read(replayed.join(".velme/artifacts").join(&name)).expect("artifact"),
            fs::read(recorded.join(".velme/artifacts").join(&name)).expect("artifact"),
            "{name}"
        );
    }
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
    // The notices, the summary and the `failed`, `ok` and `blocked` statuses all validate (AC-CLI-12).
    velme_test_support::schema::assert_cli_envelope(&envelope);
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

/// A `VELME_API_KEY` that can't be a key is `VL0405` naming it, and the build does not fall back to
/// `ANTHROPIC_API_KEY`: nothing is contacted and nothing is written (R-SEC-05, R-CLI-12).
#[test]
fn a_malformed_velme_api_key_is_vl0405_and_does_not_fall_back() {
    let dir = project("malformed-key");
    let out = velme_with(
        &dir,
        &["build", "game.velme", "--provider", "anthropic", "--model", "m"],
        None,
        &[("VELME_API_KEY", "sk bad key"), ("ANTHROPIC_API_KEY", "sk-good-key")],
    );
    assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("[VL0405]") && out.stderr.contains("`VELME_API_KEY`"),
        "{}",
        out.stderr
    );
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains("sk bad key") && !text.contains("sk-good-key"), "{text}");
    }
    assert!(
        !out.stderr.contains("Sending your plans"),
        "nothing was contacted: {}",
        out.stderr
    );
    assert!(!dir.join("velme.lock").exists());
}

/// With no usable key the Anthropic provider is still built, and a build the store can answer (a warm store, no lock)
/// takes its hits and never says `VL0405`: the identity step contacts nothing (R-SYNTH-02, R-SYNTH-25).
#[test]
fn a_warm_store_builds_with_no_api_key() {
    use velme_synth::{AnthropicConfig, SynthOptions};
    use velme_test_support::mock::{MockResponse, MockServer};
    use velme_test_support::{RecordingSleeper, mock_anthropic, program};

    let dir = project("warm-no-key");
    let parsed = program(SOURCE);
    let server = MockServer::start(
        [double(), add_one()]
            .map(|reply| MockResponse::tool_call("write_goal", &serde_json::from_str::<Value>(&reply).expect("JSON"))),
    );
    let anthropic = mock_anthropic(
        AnthropicConfig::new("claude-test"),
        &server,
        "sk-ant-mock",
        &RecordingSleeper::default(),
    );
    let mut contacts = 0;
    let report = velme_runtime::build(
        &velme_runtime::BuildInput {
            mode: velme_runtime::Mode::Build,
            program: &parsed,
            source: SOURCE,
            project: &dir,
            file: "game.velme",
            backend: Some(&anthropic),
            options: SynthOptions::default(),
            run: velme_runtime::Options::default(),
        },
        &mut || contacts += 1,
    );
    assert_eq!(report.summary.synthesized, 2, "{:?}", report.goals);
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");

    for (name, envs) in [
        ("no key", vec![]),
        ("a malformed key", vec![("VELME_API_KEY", "sk bad")]),
    ] {
        let out = velme_with(
            &dir,
            &[
                "build",
                "game.velme",
                "--provider",
                "anthropic",
                "--model",
                "claude-test",
            ],
            None,
            &envs,
        );
        assert_eq!(out.code, 0, "{name}: {}{}", out.stdout, out.stderr);
        assert!(!out.stderr.contains("VL0405"), "{name}: {}", out.stderr);
        assert!(
            !out.stderr.contains("Sending your plans"),
            "{name}: nothing is sent: {}",
            out.stderr
        );
        assert!(out.stdout.contains("from the store"), "{name}: {}", out.stdout);
        fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    }
    // A goal that does need a request says why, in the words of the setup.
    fs::remove_dir_all(dir.join(".velme")).expect("store removed");
    let cold = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "anthropic",
            "--model",
            "claude-test",
        ],
        None,
    );
    assert_eq!(cold.code, 2, "{}{}", cold.stdout, cold.stderr);
    assert!(
        cold.stderr.contains("[VL0405]") && cold.stderr.contains("needs an API key"),
        "{}",
        cold.stderr
    );
}

/// `language/12` §8.3 as `examples/beginner/double_then_add_one.velme` has it.
const WIRED_EXAMPLE: &str = include_str!("../../../examples/beginner/double_then_add_one.velme");

/// `delivery/50` §5's AC-RDM-02 program.
const WIRED_ROADMAP: &str = "language: velme/0.1

goal Double(x: Number) -> Number:
    plan: \"Multiply x by 2.\"
    check:
        - result == x * 2

goal AddOne(x: Number) -> Number:
    plan: \"Add 1 to x.\"
    check:
        - result == x + 1

goal Main(x: Number) -> Number:
    call:
        doubled = Double(x)
        result = AddOne(doubled)
    plan: |
        Return the result from the second goal.
";

/// Builds `source` as `wired.velme` in `name` on a script of two replies, `Double`'s then `AddOne`'s, and returns the
/// project: the provider is asked for those two goals only, so `Main` gets no request, and `Main(4)` runs to 9.
fn wired_main_makes_no_request(name: &str, source: &str) -> PathBuf {
    let dir = project(name);
    fs::write(dir.join("wired.velme"), source).expect("source");
    let on_x = |op: &str, k: i64| reply(op, k).replace("\"name\":\"n\"", "\"name\":\"x\"");
    let script = script(&dir, &[on_x("mul", 2), on_x("add", 1)]);
    let build = velme(&dir, &["build", "wired.velme", "--provider", "scripted"], Some(&script));
    assert_eq!(build.code, 0, "{}{}", build.stdout, build.stderr);
    assert!(
        build.stdout.contains("Main  ✓ built by the compiler"),
        "{}",
        build.stdout
    );
    assert!(build.stdout.contains("2 provider calls"), "{}", build.stdout);
    // One log line per request (R-SYNTH-23): none of them is for `Main`.
    let log = fs::read_to_string(dir.join(".velme/synth-log.jsonl")).expect("the synthesis log");
    let goals: Vec<String> = log
        .lines()
        .map(|line| {
            let line: Value = serde_json::from_str(line).expect("a JSON line");
            line["goal"].as_str().expect("a goal").to_owned()
        })
        .collect();
    assert_eq!(goals, ["Double", "AddOne"]);
    let run = velme(&dir, &["run", "wired.velme", "--goal", "Main", "--arg", "x=4"], None);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.ends_with("Main    ✓\n\nResult:\n9\n"), "{}", run.stdout);
    dir
}

/// `language/12` §8.3 builds with no request for `Main`, and `velme test` passes its example `Main(4) == 9`.
#[test]
fn ac_goal_08_the_wired_example_builds_with_no_request_for_main() {
    let dir = wired_main_makes_no_request("ac_goal_08", WIRED_EXAMPLE);
    let test = velme(&dir, &["test", "wired.velme", "--goal", "Main"], None);
    assert_eq!(test.code, 0, "{}{}", test.stdout, test.stderr);
    assert!(test.stdout.starts_with("Main  ✓ 1 example, "), "{}", test.stdout);
}

#[test]
fn ac_rdm_02_a_wired_composite_runs_with_no_synthesis_for_main() {
    wired_main_makes_no_request("ac_rdm_02", WIRED_ROADMAP);
}
