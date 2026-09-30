//! The fixture recorder (`compiler/22` R-SYNTH-43, D-94, `tooling/41` R-SEC-07): what it writes, what it leaves out, and
//! that `replay` plays it back.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use velme_ir::{Fingerprint, from_json_str, to_canonical_string};
use velme_synth::{
    AnthropicConfig, Exchange, IDENTITY_FILE, ProviderError, Recorder, Replay, ReplayIdentity, Scripted, Step,
    SynthBackend, SynthLimits, SynthRequest, build_request, fixture_path,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{RecordingSleeper, goal_id, mock_anthropic, program};

const SOURCE: &str = "language: velme/0.1

goal Rank(score: Number) -> Number:
    plan: \"Return the score. SECRET-PLAN-TEXT\"
";

const KEY: &str = "sk-ant-SENTINEL-KEY-0123456789";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
}

fn request() -> SynthRequest {
    let program = program(SOURCE);
    build_request(&program, goal_id(&program, "Rank"), SOURCE).expect("a request")
}

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("record").join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn key() -> Fingerprint {
    Fingerprint::of_bytes(b"Rank")
}

fn exchanges(dir: &Path) -> Vec<Exchange> {
    from_json_str(&fs::read_to_string(fixture_path(dir, key())).expect("a fixture")).expect("exchanges")
}

/// Every exchange that reached the provider's reply is written, in order, as canonical JSON: replies, refusals,
/// malformed and pending replies; a transport failure is not an exchange. Nothing of the prompt is kept (R-SYNTH-43).
#[test]
fn the_recorder_writes_every_exchange_that_reached_a_reply() {
    let dir = dir("exchanges");
    let scripted = Scripted::new([
        Step::Reply(r#"{"question":"Which way?"}"#.to_owned()),
        Step::Error(ProviderError::Refused("provider words".to_owned())),
        Step::Error(ProviderError::Timeout),
        Step::Error(ProviderError::Malformed("more words".to_owned())),
        Step::Error(ProviderError::Pending("ticket\n42".to_owned())),
        Step::Error(ProviderError::BackendFailed {
            reason: "crashed".to_owned(),
            stderr: String::new(),
        }),
    ]);
    let recorder = Recorder::new(Box::new(scripted), &dir);
    let identity = block_on(recorder.identify()).expect("identity");
    let provider = recorder.open(&identity).expect("a provider");
    let (request, limits) = (request(), SynthLimits::new(key()));
    let mut results = Vec::new();
    for _ in 0..6 {
        results.push(block_on(provider.complete(&request, &limits)));
    }
    assert_eq!(
        results[2],
        Err(ProviderError::Timeout),
        "the caller still sees what happened"
    );
    assert_eq!(
        results[0].as_ref().expect("a reply").reply_json,
        r#"{"question":"Which way?"}"#
    );

    let written = exchanges(&dir);
    let names: Vec<Option<&str>> = written.iter().map(|e| e.error.as_deref()).collect();
    assert_eq!(
        names,
        [
            None,
            Some("refused"),
            Some("malformed"),
            Some("pending"),
            Some("backend_failed")
        ]
    );
    assert_eq!(written[3].text.as_deref(), Some("ticket 42"));
    let hash = request.hash().expect("a hash");
    assert!(written.iter().all(|e| e.request == hash));

    let text = fs::read_to_string(fixture_path(&dir, key())).expect("a fixture");
    assert_eq!(
        text,
        to_canonical_string(&from_json_str::<Value>(&text).expect("JSON")).expect("canonical")
    );
    assert!(
        !text.contains("provider words") && !text.contains("more words"),
        "no provider prose: {text}"
    );

    let identity_text = fs::read_to_string(dir.join(IDENTITY_FILE)).expect("replay.json");
    let recorded: ReplayIdentity = from_json_str(&identity_text).expect("an identity");
    assert_eq!(
        (recorded.provider.as_str(), recorded.model_version.as_str()),
        ("scripted", "scripted")
    );
}

/// What was recorded plays back through `replay`: the same replies and error variants, under the recorded identity.
#[test]
fn a_recorded_fixture_replays() {
    let dir = dir("replays");
    let scripted = Scripted::new([
        Step::Reply(r#"{"question":"Which way?"}"#.to_owned()),
        Step::Error(ProviderError::Refused(String::new())),
        Step::Error(ProviderError::Pending("ticket 42".to_owned())),
    ]);
    let recorder = Recorder::new(Box::new(scripted), &dir);
    let identity = block_on(recorder.identify()).expect("identity");
    let provider = recorder.open(&identity).expect("a provider");
    let (request, limits) = (request(), SynthLimits::new(key()));
    for _ in 0..3 {
        let _ = block_on(provider.complete(&request, &limits));
    }

    let replay = Replay::new(&dir);
    assert_eq!(block_on(replay.identify()).expect("identity"), identity);
    let played = replay.open(&identity).expect("a provider");
    let again: Vec<_> = (0..3).map(|_| block_on(played.complete(&request, &limits))).collect();
    assert_eq!(
        again[0].as_ref().expect("a reply").reply_json,
        r#"{"question":"Which way?"}"#
    );
    assert_eq!(again[1], Err(ProviderError::Refused(String::new())));
    assert_eq!(again[2], Err(ProviderError::Pending("ticket 42".to_owned())));
}

/// A goal's whole fixture file is written again each time the goal is synthesized (R-SYNTH-43).
#[test]
fn a_goals_fixture_is_overwritten_by_the_next_recording() {
    let dir = dir("overwrite");
    let (request, limits) = (request(), SynthLimits::new(key()));
    for reply in ["one", "two"] {
        let recorder = Recorder::new(Box::new(Scripted::replies([reply])), &dir);
        let identity = block_on(recorder.identify()).expect("identity");
        block_on(
            recorder
                .open(&identity)
                .expect("a provider")
                .complete(&request, &limits),
        )
        .expect("a reply");
    }
    let written = exchanges(&dir);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].reply.as_deref(), Some("two"));
}

/// Records two exchanges from the `anthropic` provider on a mock whose responses echo the key, and returns the recording
/// directory and what the mock received.
fn record_from_anthropic(name: &str) -> (PathBuf, Vec<velme_test_support::mock::MockRequest>) {
    let dir = dir(name);
    let server = MockServer::start([
        MockResponse::tool_call("write_goal", &json!({"goal": "Rank"})),
        MockResponse::ok(
            json!({"stop_reason": "refusal", "content": [{"type": "text", "text": format!("no {KEY}")}], "usage": {}})
                .to_string(),
        ),
    ]);
    let sleeper = RecordingSleeper::default();
    let anthropic = mock_anthropic(AnthropicConfig::new("claude-test"), &server, KEY, &sleeper);
    let recorder = Recorder::new(Box::new(anthropic), &dir);
    let identity = block_on(recorder.identify()).expect("identity");
    let provider = recorder.open(&identity).expect("a provider");
    let (request, mut limits) = (request(), SynthLimits::new(key()));
    limits.timeout = Duration::from_secs(10);
    block_on(provider.complete(&request, &limits)).expect("a reply");
    assert_eq!(
        block_on(provider.complete(&request, &limits)),
        Err(ProviderError::Refused(String::new()))
    );
    (dir, server.requests())
}

/// A fixture recorded from the `anthropic` provider holds request hashes, replies and usage, and no plan text, even
/// though the prompt that was sent carried it (AC-SYNTH-38, R-SEC-07).
#[test]
fn ac_synth_38_a_recorded_fixture_holds_no_plan_text() {
    let (dir, sent) = record_from_anthropic("plan-text");
    assert_eq!(sent.len(), 2);
    assert!(
        sent[0].body.contains("SECRET-PLAN-TEXT"),
        "the prompt does carry the plan"
    );
    let written = exchanges(&dir);
    assert_eq!(written.len(), 2);
    assert_eq!(written[0].usage.output_tokens, 7);
    for entry in fs::read_dir(&dir).expect("the directory") {
        let text = fs::read_to_string(entry.expect("an entry").path()).expect("text");
        assert!(!text.contains("SECRET-PLAN-TEXT"), "{text}");
    }
    let identity_text = fs::read_to_string(dir.join(IDENTITY_FILE)).expect("replay.json");
    assert!(identity_text.contains("claude-test") && identity_text.contains("anthropic"));
}

/// No key, header or provider prose reaches a fixture, even when the provider's own response echoes the key
/// (AC-SYNTH-09, R-SEC-06, R-SEC-07).
#[test]
fn ac_synth_09_no_key_string_appears_in_a_recorded_fixture() {
    let (dir, sent) = record_from_anthropic("key");
    assert_eq!(sent[0].headers["x-api-key"], KEY, "the key was sent as a header");
    for entry in fs::read_dir(&dir).expect("the directory") {
        let path = entry.expect("an entry").path();
        let text = fs::read_to_string(&path).expect("text");
        for forbidden in [KEY, "SENTINEL", "x-api-key", "sk-ant-"] {
            assert!(
                !text.contains(forbidden),
                "{} holds {forbidden}: {text}",
                path.display()
            );
        }
    }
}

/// Fixtures and `replay.json` are written and read as the artifact store's files are: a link at either path is never
/// written through or followed, and is `VL0901` (R-SYNTH-43, R-ART-09, R-ART-10).
#[cfg(unix)]
#[test]
fn r_synth_43_a_link_in_place_of_a_fixture_is_vl0901_and_is_never_followed() {
    use std::os::unix::fs::symlink;
    use velme_diagnostics::{Code, Span};
    use velme_synth::provider_diagnostic;

    let dir = dir("links");
    fs::create_dir_all(&dir).expect("directory");
    let outside = dir.join("outside.txt");
    fs::write(&outside, "untouched").expect("outside file");
    symlink(&outside, fixture_path(&dir, key())).expect("a link");
    symlink(&outside, dir.join(IDENTITY_FILE)).expect("a link");
    let file_error = |error: ProviderError| {
        assert!(matches!(error, ProviderError::File { .. }), "{error:?}");
        assert_eq!(
            provider_diagnostic(&error, "test", "Rank", Span::default()).code,
            Code::FileError
        );
    };

    // Recording refuses both, leaving the file behind the links as it was.
    let recorder = Recorder::new(Box::new(Scripted::new([Step::Reply("{}".to_owned())])), &dir);
    file_error(block_on(recorder.identify()).expect_err("replay.json is a link"));
    fs::remove_file(dir.join(IDENTITY_FILE)).expect("link removed");
    let identity = block_on(recorder.identify()).expect("identity");
    let provider = recorder.open(&identity).expect("a provider");
    file_error(block_on(provider.complete(&request(), &SynthLimits::new(key()))).expect_err("the fixture is a link"));
    assert_eq!(fs::read_to_string(&outside).expect("outside file"), "untouched");

    // Replaying does not follow them either, though the file behind holds a plausible fixture.
    fs::write(&outside, "[]").expect("outside file");
    let replay = Replay::new(&dir);
    let provider = replay.open(&identity).expect("a provider");
    file_error(block_on(provider.complete(&request(), &SynthLimits::new(key()))).expect_err("the fixture is a link"));
    fs::remove_file(dir.join(IDENTITY_FILE)).expect("replay.json removed");
    symlink(&outside, dir.join(IDENTITY_FILE)).expect("a link");
    file_error(block_on(replay.identify()).expect_err("replay.json is a link"));

    // A fixture past the bound is not read.
    fs::remove_file(fixture_path(&dir, key())).expect("link removed");
    fs::write(fixture_path(&dir, key()), vec![b' '; 16 * 1024 * 1024 + 1]).expect("a big file");
    file_error(block_on(provider.complete(&request(), &SynthLimits::new(key()))).expect_err("too long"));
}
