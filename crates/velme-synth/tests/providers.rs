//! The `scripted` and `replay` providers (`compiler/22` R-SYNTH-05, R-SYNTH-43).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::future::Future;
use std::path::PathBuf;

use velme_ir::{Fingerprint, to_canonical_string};
use velme_synth::{
    Exchange, FixtureUsage, Identity, ProviderError, Replay, ReplayIdentity, Scripted, Step, SynthBackend, SynthLimits,
    SynthProvider, SynthRequest, build_request, fixture_path, prompt_version,
};
use velme_test_support::{PanicProvider, goal_id, program};

const SOURCE: &str = "language: velme/0.1

goal Rank(score: Number) -> Number:
    plan: \"Return the score. SECRET-PLAN-TEXT\"
";

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

fn key(seed: u8) -> Fingerprint {
    Fingerprint::of_bytes(&[seed])
}

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("replay").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("directory");
    dir
}

fn identity() -> ReplayIdentity {
    ReplayIdentity {
        provider: "anthropic".to_owned(),
        model_version: "claude-test".to_owned(),
        input_version: "prompt-1:abc".to_owned(),
        backend: None,
        retry_history: None,
        reply_format: None,
    }
}

fn write(dir: &std::path::Path, name: &str, text: &impl serde::Serialize) {
    fs::write(dir.join(name), to_canonical_string(text).expect("canonical")).expect("written");
}

fn reply_of(request: &SynthRequest, reply: &str) -> Exchange {
    Exchange {
        request: request.hash().expect("hash"),
        reply: Some(reply.to_owned()),
        error: None,
        text: None,
        usage: FixtureUsage {
            input_tokens: 10,
            output_tokens: 20,
            ..FixtureUsage::default()
        },
    }
}

fn expect_unavailable(result: Result<velme_synth::SynthReply, ProviderError>) -> String {
    match result {
        Err(ProviderError::Unavailable(text)) => text,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// `scripted` answers in order, reports what it was asked, and reports an empty queue as `Unavailable`.
#[test]
fn scripted_answers_in_order_and_remembers_the_requests() {
    let scripted = Scripted::new([
        Step::Reply("first".to_owned()),
        Step::Error(ProviderError::Refused("no".to_owned())),
        Step::Reply("third".to_owned()),
    ]);
    let request = request();
    let limits = SynthLimits::new(key(1));
    let first = block_on(scripted.complete(&request, &limits)).expect("a reply");
    assert_eq!(first.reply_json, "first");
    assert!(matches!(
        block_on(scripted.complete(&request, &limits)),
        Err(ProviderError::Refused(_))
    ));
    assert_eq!(
        block_on(scripted.complete(&request, &limits))
            .expect("a reply")
            .reply_json,
        "third"
    );
    assert!(matches!(
        block_on(scripted.complete(&request, &limits)),
        Err(ProviderError::Unavailable(_))
    ));
    assert_eq!(scripted.calls(), 4);
    assert_eq!(scripted.requests()[0], request);
    assert_eq!(scripted.limits()[0].synthesis_key, key(1));
    assert_eq!(scripted.remaining(), 0);
}

/// The identity step contacts nothing for `scripted`, and the provider it opens shares the queue (R-SYNTH-25).
#[test]
fn scripted_identifies_itself_and_opens_a_provider_on_the_same_queue() {
    let scripted = Scripted::replies(["only"]);
    let identity = block_on(scripted.identify()).expect("identity");
    assert_eq!(
        identity,
        Identity {
            provider: "scripted".to_owned(),
            model: "scripted".to_owned(),
            input_version: prompt_version(),
            backend: None,
        }
    );
    let provider = scripted.open(&identity).expect("a provider");
    assert_eq!(
        (provider.id(), provider.model(), provider.input_version()),
        ("scripted", "scripted", prompt_version().as_str())
    );
    let reply = block_on(provider.complete(&request(), &SynthLimits::new(key(1)))).expect("a reply");
    assert_eq!(reply.reply_json, "only");
    assert_eq!((scripted.calls(), scripted.remaining()), (1, 0));
}

/// A script file is a JSON array of replies and `{"error": variant}` entries (D-94).
#[test]
fn a_script_file_is_a_json_array_of_replies_and_errors() {
    let scripted = Scripted::from_script(
        r#"[{"goal": "Rank", "n": 1.50}, "not json at all", {"error": "refused"}, {"error": "pending", "text": "ticket 42"}]"#,
    )
    .expect("a script");
    let limits = SynthLimits::new(key(1));
    let request = request();
    let reply = |scripted: &Scripted| block_on(scripted.complete(&request, &limits));
    assert_eq!(
        reply(&scripted).expect("a reply").reply_json,
        r#"{"goal":"Rank","n":1.5}"#
    );
    assert_eq!(reply(&scripted).expect("a reply").reply_json, "not json at all");
    assert_eq!(
        reply(&scripted).expect_err("an error"),
        ProviderError::Refused(String::new())
    );
    assert_eq!(
        reply(&scripted).expect_err("an error"),
        ProviderError::Pending("ticket 42".to_owned())
    );
    for bad in [
        r#"[{"error": "refused", "extra": 1}]"#,
        r#"[{"error": "refused", "text": "x"}]"#,
        r#"[{"error": "pending", "text": 3}]"#,
        "{}",
        "[{\"error\": \"exploded\"}]",
        "[{\"error\": 3}]",
        "[1,",
    ] {
        assert!(Scripted::from_script(bad).is_err(), "{bad} is not a script");
    }
}

/// `replay` reports the recorded build's identity, so the replayed build computes the same keys (R-SYNTH-43).
#[test]
fn replay_reports_the_identity_it_recorded() {
    let dir = dir("identity");
    let replay = Replay::new(&dir);
    let missing = block_on(replay.identify()).expect_err("no replay.json");
    assert!(matches!(missing, ProviderError::Unavailable(text) if text.contains("VELME_SYNTH_RECORD=1")));
    write(&dir, "replay.json", &identity());
    let found = block_on(replay.identify()).expect("identity");
    assert_eq!(
        found,
        Identity {
            provider: "anthropic".to_owned(),
            model: "claude-test".to_owned(),
            input_version: "prompt-1:abc".to_owned(),
            backend: None,
        }
    );
    let provider = replay.open(&found).expect("a provider");
    assert_eq!(
        (provider.id(), provider.model(), provider.input_version()),
        ("anthropic", "claude-test", "prompt-1:abc")
    );
}

/// `replay` plays a goal's exchanges back in order: replies with their usage, and error variants.
#[test]
fn replay_plays_the_recorded_exchanges_in_order() {
    let dir = dir("plays");
    let request = request();
    let key = key(2);
    let exchanges = vec![
        reply_of(&request, "{\"bad\":true}"),
        Exchange {
            error: Some("pending".to_owned()),
            text: Some("ticket 42".to_owned()),
            reply: None,
            ..reply_of(&request, "")
        },
    ];
    write(&dir, "replay.json", &identity());
    write(
        &dir,
        fixture_path(&dir, key)
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a name"),
        &exchanges,
    );
    let replay = Replay::new(&dir);
    let identity = block_on(replay.identify()).expect("identity");
    let provider = replay.open(&identity).expect("a provider");
    let limits = SynthLimits::new(key);
    let first = block_on(provider.complete(&request, &limits)).expect("a reply");
    assert_eq!(first.reply_json, "{\"bad\":true}");
    assert_eq!((first.usage.input_tokens, first.usage.output_tokens), (10, 20));
    assert_eq!(
        block_on(provider.complete(&request, &limits)).expect_err("an error"),
        ProviderError::Pending("ticket 42".to_owned())
    );
}

/// A fixture holds request hashes, replies and usage but no plan text; a request that differs from the recorded one, or
/// one more than were recorded, is `Unavailable` (`VL0404`) naming the key and the attempt, with the re-record hint
/// (R-SYNTH-43, D-94).
#[test]
fn ac_synth_38_replay_refuses_an_edited_request_and_an_extra_one() {
    let dir = dir("refuses");
    let request = request();
    let key = key(3);
    write(&dir, "replay.json", &identity());
    let name = fixture_path(&dir, key);
    write(
        &dir,
        name.file_name().and_then(|n| n.to_str()).expect("a name"),
        &vec![reply_of(&request, "one")],
    );
    let stored = fs::read_to_string(&name).expect("the fixture");
    assert!(stored.contains(&request.hash().expect("hash").to_string()));
    assert!(!stored.contains("SECRET-PLAN-TEXT"), "the fixture holds plan text");
    let replay = Replay::new(&dir);
    let identity = block_on(replay.identify()).expect("identity");
    let limits = SynthLimits::new(key);

    let provider = replay.open(&identity).expect("a provider");
    let mut edited = request.clone();
    edited.plan.push_str(" Edited.");
    let text = expect_unavailable(block_on(provider.complete(&edited, &limits)));
    assert!(text.contains(&key.to_string()) && text.contains("attempt 1"), "{text}");
    assert!(
        text.contains("differs") && text.ends_with("re-record the fixture with VELME_SYNTH_RECORD=1"),
        "{text}"
    );

    let provider = replay.open(&identity).expect("a provider");
    assert_eq!(
        block_on(provider.complete(&request, &limits))
            .expect("a reply")
            .reply_json,
        "one"
    );
    let text = expect_unavailable(block_on(provider.complete(&request, &limits)));
    assert!(
        text.contains(&key.to_string()) && text.contains("attempt 2") && text.contains("more attempts"),
        "{text}"
    );

    let other = SynthLimits::new(self::key(4));
    let text = expect_unavailable(block_on(
        replay.open(&identity).expect("a provider").complete(&request, &other),
    ));
    assert!(text.contains("there is no fixture"), "{text}");
}

/// A provider that panics on use stands in wherever no provider may be contacted (AC-CMP-02, AC-SYNTH-01).
#[test]
#[should_panic(expected = "a provider was called")]
fn the_panic_provider_panics_when_called() {
    let _ = block_on(PanicProvider.complete(&request(), &SynthLimits::new(key(1))));
}

/// `text` belongs to a `pending` exchange only; anywhere else the fixture is malformed.
#[test]
fn replay_rejects_text_outside_a_pending_exchange() {
    let dir = dir("text");
    let request = request();
    let key = key(5);
    write(&dir, "replay.json", &identity());
    let bad = Exchange {
        text: Some("x".to_owned()),
        ..reply_of(&request, "one")
    };
    write(
        &dir,
        fixture_path(&dir, key)
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a name"),
        &vec![bad],
    );
    let replay = Replay::new(&dir);
    let identity = block_on(replay.identify()).expect("identity");
    let text = expect_unavailable(block_on(
        replay
            .open(&identity)
            .expect("a provider")
            .complete(&request, &SynthLimits::new(key)),
    ));
    assert!(text.contains("`text` without a `pending` error"), "{text}");
}
