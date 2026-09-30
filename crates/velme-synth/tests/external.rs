//! The `external` provider against the test backend (`compiler/22` §3.2, R-SYNTH-26..29, R-SYNTH-41, D-42, D-45, D-98):
//! real child processes, with the backend's own misbehaviour on request. Only the two timeout tests wait on the clock.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use velme_synth::{
    External, ExternalCommand, ExternalConfig, ExternalMessage, Identity, ProviderError, REQUEST_VERSION, SynthBackend,
    SynthLimits, SynthRequest, build_request,
};
use velme_test_support::{backend_words, goal_id, program, repo};

const SOURCE: &str = "language: velme/0.1

type Player:
    name: Text
    score: Number

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise. SECRET-PLAN-TEXT
    check:
        - result == \"Gold\" or result == \"Silver\" or result == \"Bronze\"
    examples:
        - FindBadge(Player(name: \"Lina\", score: 820)) == \"Silver\"
";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
}

fn request() -> SynthRequest {
    let program = program(SOURCE);
    build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("a request")
}

fn limits() -> SynthLimits {
    SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"goal"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("external").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// The backend run in `root` with `args` and a timeout of `timeout`.
fn external(root: &Path, args: &[&str], timeout: Duration) -> External {
    let command = ExternalCommand::resolve(&backend_words(args), root).expect("a command");
    let mut config = ExternalConfig::new(command, root);
    config.timeout = timeout;
    External::new(config)
}

const PATIENT: Duration = Duration::from_secs(30);

/// A backend that answers `reply` (a JSON text) to every `FindBadge`, from a directory holding that one file.
fn replying(name: &str, reply: &str) -> (External, PathBuf) {
    let dir = scratch(name);
    fs::write(dir.join("FindBadge.json"), reply).expect("reply file");
    let backend = external(&dir, &["--dir", dir.to_str().expect("utf-8")], PATIENT);
    (backend, dir)
}

fn complete(backend: &External, request: &SynthRequest) -> Result<String, ProviderError> {
    let identity = Identity {
        provider: "external".to_owned(),
        model: "v".to_owned(),
        input_version: REQUEST_VERSION.to_owned(),
        backend: Some("test".to_owned()),
    };
    let provider = backend.open(&identity).expect("a provider");
    block_on(provider.complete(request, &limits())).map(|reply| reply.reply_json)
}

fn failure(result: Result<String, ProviderError>) -> String {
    match result.expect_err("a failure") {
        ProviderError::BackendFailed { reason, stderr } if stderr.is_empty() => reason,
        ProviderError::BackendFailed { reason, stderr } => format!("{reason}\n{stderr}"),
        other => panic!("not a backend failure: {other:?}"),
    }
}

/// `describe` names the backend and its version, which are the model and the request version of the identity
/// (R-SYNTH-25, R-SYNTH-26).
#[test]
fn describe_gives_the_backend_name_and_version() {
    let dir = scratch("describe");
    let backend = external(&dir, &["--describe", "  my   backend ", "v  2"], PATIENT);
    let identity = block_on(backend.identify()).expect("identity");
    assert_eq!(identity.provider, "external");
    assert_eq!(identity.backend.as_deref(), Some("my backend"));
    assert_eq!(identity.model, "v 2");
    assert_eq!(identity.input_version, REQUEST_VERSION);
}

/// A name or version that is empty, over 128 characters, blank, or holding a control, format or bidi character, or a
/// reply with anything else in it, is a backend failure (R-SYNTH-26, D-98).
#[test]
fn a_describe_reply_that_breaks_the_rules_is_a_backend_failure() {
    let dir = scratch("describe-bad");
    for (name, version) in [
        ("", "v"),
        ("n", ""),
        (" \u{7} ", "v"),
        // Control, format and bidi characters are refused, not cleaned away (R-SYNTH-26, D-47).
        ("my\u{1b}[1m backend", "v"),
        ("n", "v\n2"),
        ("n\u{202e}", "v"),
        ("n", "v\u{200b}2"),
        ("\u{feff}n", "v"),
        (&"n".repeat(129), "v"),
        ("n", &"v".repeat(129)),
    ] {
        let backend = external(&dir, &["--describe", name, version], PATIENT);
        let error = block_on(backend.identify()).expect_err("not a description");
        assert!(
            matches!(&error, ProviderError::BackendFailed { .. }),
            "{name:?} {version:?}: {error:?}"
        );
    }
    let edge = external(&dir, &["--describe", &"n".repeat(128), &"v".repeat(128)], PATIENT);
    assert!(block_on(edge.identify()).is_ok(), "128 characters are allowed");
    // A `synthesize`-shaped answer, garbage and a wrong-typed field.
    let (backend, _) = replying("describe-shape", "{}");
    let garbage = external(&dir, &["--mode", "garbage", "--on", "describe"], PATIENT);
    for backend in [garbage, backend] {
        let error = block_on(backend.identify());
        if let Err(error) = error {
            assert!(matches!(error, ProviderError::BackendFailed { .. }), "{error:?}");
        }
    }
}

/// A `describe` reply may carry keys Velme doesn't know; only the two fields matter (R-SYNTH-26).
#[test]
fn unknown_describe_keys_are_ignored() {
    let dir = scratch("describe-extra");
    let backend = external(&dir, &["--describe", "queue", "v1", "--describe-extra"], PATIENT);
    let identity = block_on(backend.identify()).expect("identity");
    assert_eq!(identity.backend.as_deref(), Some("queue"));
}

/// The text of an `{"error"}` reply is cleaned, collapsed and bounded like any other untrusted line (R-SYNTH-28).
#[test]
fn an_error_reply_is_cleaned_and_bounded() {
    let text = format!("bad\u{1b}[31m\n\tthing {}", "x".repeat(1000));
    let (backend, _) = replying("error-text", &json!({"error": text}).to_string());
    let error = complete(&backend, &request()).expect_err("a failure");
    let ProviderError::BackendFailed { reason, stderr } = error else {
        panic!("not a backend failure");
    };
    assert!(stderr.is_empty());
    assert!(reason.starts_with("it reported an error: bad thing xxx"), "{reason}");
    assert!(!reason.contains(['\u{1b}', '\n', '\t']));
    assert!(reason.chars().count() < 340, "{}", reason.chars().count());
}

/// An `ir` reply is the IR object; a `question` reply is the question object; `pending` keeps its text; an `error` reply
/// and a reply of no known kind are backend failures (R-SYNTH-27, R-SYNTH-28, R-SYNTH-41).
#[test]
fn a_reply_is_one_of_four_kinds() {
    let ir = json!({"ir_version": "0.1", "goal": "FindBadge", "note": 1.50});
    let (backend, _) = replying("kind-ir", &ir.to_string());
    assert_eq!(
        complete(&backend, &request()).expect("an ir reply"),
        r#"{"goal":"FindBadge","ir_version":"0.1","note":1.5}"#
    );
    let (backend, _) = replying("kind-question", r#"{"question": "Which way?"}"#);
    assert_eq!(
        complete(&backend, &request()).expect("a question"),
        r#"{"question":"Which way?"}"#
    );
    let (backend, _) = replying("kind-pending", r#"{"pending": "ticket 42"}"#);
    assert_eq!(
        complete(&backend, &request()),
        Err(ProviderError::Pending("ticket 42".to_owned()))
    );
    let (backend, _) = replying("kind-error", r#"{"error": "no idea"}"#);
    assert!(failure(complete(&backend, &request())).contains("no idea"));
    for (name, reply) in [
        ("kind-empty", "{}"),
        ("kind-two", r#"{"ir": {}, "question": "?"}"#),
        ("kind-unknown", r#"{"result": 1}"#),
        ("kind-ir-string", r#"{"ir": "text"}"#),
        ("kind-pending-number", r#"{"pending": 7}"#),
        ("kind-array", "[1]"),
    ] {
        let (backend, _) = replying(name, reply);
        let text = failure(complete(&backend, &request()));
        assert!(!text.is_empty(), "{name}");
    }
}

/// A non-zero exit is a failure whose notes hold the last words of stderr, cleaned; so are stdout that isn't JSON and
/// stdout past 2 MiB (AC-SYNTH-16, R-SYNTH-28).
#[test]
fn an_exit_garbage_and_a_flood_are_backend_failures_with_the_tail_of_stderr() {
    let dir = scratch("failures");
    let exited = failure(complete(&external(&dir, &["--mode", "exit"], PATIENT), &request()));
    assert_eq!(exited, "it exited with status 3\nbackend exploded : it is a test");
    let garbage = failure(complete(&external(&dir, &["--mode", "garbage"], PATIENT), &request()));
    assert!(garbage.starts_with("its output wasn't one JSON object"), "{garbage}");
    // Every failure carries the tail of stderr, not only a non-zero exit (R-SYNTH-28).
    assert!(
        garbage.ends_with("garbage mode: printing what is not JSON"),
        "{garbage}"
    );
    let described = block_on(external(&dir, &["--mode", "garbage", "--on", "describe"], PATIENT).identify());
    assert!(
        matches!(&described, Err(ProviderError::BackendFailed { stderr, .. }) if stderr.contains("garbage mode")),
        "{described:?}"
    );
    let flood = failure(complete(&external(&dir, &["--mode", "huge"], PATIENT), &request()));
    assert!(flood.starts_with("it wrote more than 2 MiB"), "{flood}");
}

/// A backend that outlives the timeout is a failure, `describe` included, and the whole process group is killed
/// (R-SYNTH-28, D-98).
#[test]
fn a_backend_that_outlives_the_timeout_is_stopped_with_its_whole_group() {
    let dir = scratch("timeout");
    let quick = Duration::from_millis(400);
    let started = Instant::now();
    let text = failure(complete(&external(&dir, &["--mode", "hang"], quick), &request()));
    assert!(text.starts_with("it took longer than"), "{text}");
    let described = block_on(external(&dir, &["--mode", "hang", "--on", "describe"], quick).identify());
    assert!(
        matches!(described, Err(ProviderError::BackendFailed { .. })),
        "{described:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the hang was not waited out"
    );
    #[cfg(unix)]
    {
        let pidfile = dir.join("pid");
        let group = external(
            &dir,
            &["--mode", "hang-group", "--pidfile", pidfile.to_str().expect("utf-8")],
            quick,
        );
        failure(complete(&group, &request()));
        let pid = fs::read_to_string(&pidfile).expect("the grandchild's pid");
        assert!(gone(&pid), "the grandchild {pid} is still running");
    }
}

/// Whether the process `pid` is gone. Signal 0 asks only whether it exists; a killed one is reaped by init, so it is
/// given a moment.
#[cfg(unix)]
fn gone(pid: &str) -> bool {
    let alive = || {
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    !alive()
}

/// A backend that replies and exits but leaves a child holding its stdout open is not a hang: the reply is accepted at
/// once, no timeout is reported, and the child is killed with the group (R-SYNTH-28).
#[cfg(unix)]
#[test]
fn a_reply_is_accepted_when_a_leftover_child_holds_the_pipe_and_the_child_is_killed() {
    let dir = scratch("sleeper");
    fs::write(dir.join("FindBadge.json"), r#"{"question":"Which badge?"}"#).expect("reply file");
    let pidfile = dir.join("pid");
    let backend = external(
        &dir,
        &[
            "--dir",
            dir.to_str().expect("utf-8"),
            "--sleeper",
            "--pidfile",
            pidfile.to_str().expect("utf-8"),
        ],
        PATIENT,
    );
    let started = Instant::now();
    let reply = complete(&backend, &request()).expect("the reply is accepted");
    assert!(reply.contains("Which badge?"), "{reply}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the sleeper was waited for"
    );
    let pid = fs::read_to_string(&pidfile).expect("the sleeper's pid");
    assert!(gone(&pid), "the sleeper {pid} is still running");
}

/// A command that can't be started is a backend failure, not a panic.
#[test]
fn a_command_that_cannot_start_is_a_backend_failure() {
    let dir = scratch("nostart");
    let missing = dir.join("no-such-program");
    let command = ExternalCommand::resolve(&[missing.to_string_lossy().into_owned()], &dir).expect("absolute");
    let backend = External::new(ExternalConfig::new(command, &dir));
    assert_eq!(failure(complete(&backend, &request())), "it couldn't be started");
    assert!(matches!(
        block_on(backend.identify()),
        Err(ProviderError::BackendFailed { .. })
    ));
}

/// The `synthesize` message for a golden goal is the snapshot, parses back through the committed types of the request
/// schema, and holds no file path, environment value or key (AC-SYNTH-14, R-SYNTH-26).
#[test]
fn ac_synth_14_the_synthesize_message_is_the_snapshot_and_holds_nothing_local() {
    let dir = scratch("message");
    let capture = dir.join("stdin.json");
    let backend = external(
        &dir,
        &[
            "--dir",
            dir.to_str().expect("utf-8"),
            "--capture",
            capture.to_str().expect("utf-8"),
        ],
        PATIENT,
    );
    // The backend has no reply for the goal, so this is an `error`; the message it received is what is checked.
    let _ = complete(&backend, &request());
    let text = fs::read_to_string(&capture).expect("the message the backend received");
    let message: ExternalMessage = velme_ir::from_json_str(&text).expect("the committed message types accept it");
    assert_eq!(message, ExternalMessage::synthesize(request()));
    let mut shown = serde_json::to_value(&message).expect("serializes");
    assert_eq!(shown["kind"], "synthesize");
    assert_eq!(shown["request_version"], REQUEST_VERSION);
    shown["request"]["output_schema"] = json!("<reply schema>");
    insta::assert_snapshot!(
        "external_synthesize_message",
        serde_json::to_string_pretty(&shown).expect("prints")
    );
    let mut forbidden = vec![
        dir.to_string_lossy().into_owned(),
        repo("").to_string_lossy().into_owned(),
        env!("CARGO_MANIFEST_DIR").to_owned(),
        "sk-".to_owned(),
        "API_KEY".to_owned(),
    ];
    forbidden.extend(
        ["HOME", "USER", "PATH"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .filter(|v| v.len() > 3),
    );
    for value in forbidden {
        assert!(!text.contains(&value), "the message holds {value}");
    }
}
