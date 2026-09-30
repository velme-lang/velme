//! The `ollama` provider against a mock server (`compiler/22` R-SYNTH-05, R-SYNTH-24, R-SYNTH-25, D-41, D-98): no
//! network, and no wall-clock wait except the one timeout test.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use velme_synth::{
    Identity, Ollama, OllamaConfig, ProviderError, SynthBackend, SynthLimits, SynthRequest, build_request,
    prompt_version, render_with,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{RecordingSleeper, goal_id, program};

const SOURCE: &str = "language: velme/0.1

goal Rank(score: Number) -> Number:
    plan: \"Return the score. SECRET-PLAN-TEXT\"
";

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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

fn limits() -> SynthLimits {
    SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"goal"))
}

/// A backend for `model` on `responses`, with the server and the sleeper to inspect afterwards.
fn mock(model: &str, responses: impl IntoIterator<Item = MockResponse>) -> (Ollama, MockServer, RecordingSleeper) {
    let server = MockServer::start(responses);
    let sleeper = RecordingSleeper::default();
    let mut config = OllamaConfig::new(model);
    config.url = server.url().to_owned();
    let backend = Ollama::new(config).with_sleeper(Arc::new(sleeper.clone()));
    (backend, server, sleeper)
}

fn goal_reply() -> Value {
    json!({"goal": "Rank", "note": 1.5})
}

fn identify(backend: &Ollama) -> Result<Identity, ProviderError> {
    block_on(backend.identify())
}

/// The chat request sets the reply schema as `format`, temperature 0 and no streaming; the identity is
/// `<model>@<digest>` with the digest kept whole, and the reply and usage come back (AC-SYNTH-11, R-SYNTH-05,
/// R-SYNTH-24).
#[test]
fn ac_synth_11_the_chat_request_carries_the_schema_and_no_streaming() {
    let (backend, server, sleeper) = mock(
        "llama3",
        [
            MockResponse::ollama_tags(&[("other:latest", "sha256:other"), ("llama3:latest", DIGEST)]),
            MockResponse::ollama_chat(&goal_reply()),
        ],
    );
    let identity = identify(&backend).expect("identity");
    assert_eq!(identity.provider, "ollama");
    assert_eq!(identity.model, format!("llama3:latest@{DIGEST}"));
    assert_eq!(
        identity.input_version,
        OllamaConfig::new("llama3").options.input_version(&prompt_version())
    );
    assert_eq!(identity.backend, None);
    let provider = backend.open(&identity).expect("a provider");
    assert_eq!((provider.id(), provider.model()), ("ollama", identity.model.as_str()));
    let request = request();
    let reply = block_on(provider.complete(&request, &limits())).expect("a reply");
    assert_eq!(reply.reply_json, r#"{"goal":"Rank","note":1.5}"#);
    assert_eq!((reply.usage.input_tokens, reply.usage.output_tokens), (13, 9));
    assert!(sleeper.waits().is_empty());
    let sent = server.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].path, "/api/tags");
    assert_eq!(sent[1].path, "/api/chat");
    assert!(!sent[1].headers.contains_key("authorization") && !sent[1].headers.contains_key("x-api-key"));
    let body = sent[1].json();
    assert_eq!(body["model"], "llama3:latest");
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["format"], request.output_schema);
    assert_eq!(body["options"]["temperature"], 0);
    assert_eq!(body["options"]["num_predict"], 8192);
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    let prompt = render_with(&request, &OllamaConfig::new("llama3").options.prompt()).expect("a prompt");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], prompt.first_turn());
}

/// A model with a tag matches by that tag alone, and one without matches `:latest` (R-SYNTH-24).
#[test]
fn a_model_name_is_normalised_before_it_is_looked_up() {
    let tags = || MockResponse::ollama_tags(&[("llama3:8b", "sha256:eight"), ("llama3:latest", "sha256:latest")]);
    let (tagged, _server, _) = mock("llama3:8b", [tags()]);
    assert_eq!(identify(&tagged).expect("identity").model, "llama3:8b@sha256:eight");
    let (bare, _server, _) = mock("llama3", [tags()]);
    assert_eq!(identify(&bare).expect("identity").model, "llama3:latest@sha256:latest");
    let (missing, _server, _) = mock("llama3:70b", [tags()]);
    assert_eq!(identify(&missing), Err(ProviderError::NotConfigured));
}

/// An unreachable server is `Unavailable` after the transport waits; a model it lacks, in the list or at chat time, is
/// `NotConfigured`; a list that is not one is `Unavailable` (AC-SYNTH-13, R-SYNTH-24).
#[test]
fn ac_synth_13_an_unreachable_server_is_unavailable_and_a_missing_model_is_not_configured() {
    let server = MockServer::start([]);
    let sleeper = RecordingSleeper::default();
    let mut config = OllamaConfig::new("llama3");
    config.url = server.url().to_owned();
    let backend = Ollama::new(config).with_sleeper(Arc::new(sleeper.clone()));
    drop(server);
    let error = identify(&backend).expect_err("nothing there");
    assert!(matches!(&error, ProviderError::Unavailable(_)), "{error:?}");
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);

    let (backend, _server, _) = mock("llama3", [MockResponse::ollama_tags(&[])]);
    assert_eq!(identify(&backend), Err(ProviderError::NotConfigured));

    let (backend, _server, _) = mock(
        "llama3",
        [
            MockResponse::ollama_tags(&[("llama3:latest", DIGEST)]),
            MockResponse::status(404, r#"{"error":"model 'llama3' not found"}"#),
        ],
    );
    let identity = identify(&backend).expect("identity");
    let provider = backend.open(&identity).expect("a provider");
    assert_eq!(
        block_on(provider.complete(&request(), &limits())),
        Err(ProviderError::NotConfigured)
    );

    for body in [
        "not json",
        r#"{"nothing": []}"#,
        r#"{"models":[{"name":"llama3:latest","digest":""}]}"#,
    ] {
        let (backend, _server, _) = mock("llama3", [MockResponse::ok(body)]);
        let error = identify(&backend).expect_err("an unusable list");
        assert!(matches!(&error, ProviderError::Unavailable(_)), "{body}: {error:?}");
    }
}

/// A reply that is not one JSON object, is cut off, nests too deeply or repeats a key is `Malformed`, and an oversized
/// body is too; none carries the server's text (D-98, D-93).
#[test]
fn a_reply_that_is_not_a_bounded_object_is_malformed() {
    let chat = |content: &str, done_reason: &str| {
        MockResponse::ok(
            json!({"message": {"role": "assistant", "content": content}, "done": true, "done_reason": done_reason})
                .to_string(),
        )
    };
    let deep = format!("{}{}", "[".repeat(600), "]".repeat(600));
    let (backend, _server, _) = mock(
        "m",
        [
            MockResponse::ollama_tags(&[("m:latest", DIGEST)]),
            chat("prose SERVER-WORDS", "stop"),
            chat("[1, 2]", "stop"),
            chat(r#"{"a": 1, "a": 2}"#, "stop"),
            chat(&deep, "stop"),
            chat(r#"{"goal": "Rank"}"#, "length"),
            MockResponse::ok(r#"{"done": true}"#),
            MockResponse::ok("x".repeat(9 * 1024 * 1024)),
        ],
    );
    let provider = backend
        .open(&identify(&backend).expect("identity"))
        .expect("a provider");
    for _ in 0..7 {
        let error = block_on(provider.complete(&request(), &limits())).expect_err("malformed");
        assert!(matches!(&error, ProviderError::Malformed(_)), "{error:?}");
        assert!(!format!("{error:?}").contains("SERVER-WORDS"));
    }
}

/// A 429 and a 5xx wait on the injected clock inside one call, and a 400 is a bug of Velme's (R-SYNTH-12, D-95).
#[test]
fn rate_limits_and_server_errors_are_retried_on_the_injected_clock() {
    let (backend, server, sleeper) = mock(
        "m",
        [
            MockResponse::ollama_tags(&[("m:latest", DIGEST)]),
            MockResponse::status(429, ""),
            MockResponse::status(503, ""),
            MockResponse::ollama_chat(&goal_reply()),
            MockResponse::status(400, "bad"),
        ],
    );
    let provider = backend
        .open(&identify(&backend).expect("identity"))
        .expect("a provider");
    block_on(provider.complete(&request(), &limits())).expect("a reply after two waits");
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);
    assert_eq!(server.requests().len(), 4);
    let error = block_on(provider.complete(&request(), &limits())).expect_err("rejected");
    assert!(matches!(error, ProviderError::Internal(_)), "{error:?}");
}

/// A chat that gets no answer within `timeout` is `Timeout` at once, not retried (R-SYNTH-12, D-110).
#[test]
fn a_chat_that_gets_no_answer_times_out_without_a_retry() {
    let (backend, _server, sleeper) = mock(
        "m",
        [
            MockResponse::ollama_tags(&[("m:latest", DIGEST)]),
            MockResponse::hang(),
            MockResponse::hang(),
        ],
    );
    let provider = backend
        .open(&identify(&backend).expect("identity"))
        .expect("a provider");
    let mut limits = limits();
    limits.timeout = Duration::from_millis(150);
    assert_eq!(
        block_on(provider.complete(&request(), &limits)),
        Err(ProviderError::Timeout)
    );
    assert!(sleeper.waits().is_empty());
}
