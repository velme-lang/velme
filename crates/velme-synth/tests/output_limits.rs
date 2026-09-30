//! `max_output_tokens` per provider and the transport retries of a generation (`compiler/22` R-SYNTH-12, §8, D-110), through
//! the retry loop against mock servers: no network. The one timeout is a second of wall time.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};
use velme_diagnostics::Code;
use velme_ir::{Fingerprint, contract_key};
use velme_synth::{
    AnthropicConfig, Ollama, OllamaConfig, Outcome, Session, SynthBackend, SynthOptions, Task, synthesize,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{LeafRunner, RecordingSleeper, goal_id, mock_anthropic, program};

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    examples:
        - Double(2) == 4
";

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const KEY: &str = "sk-test-key";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
}

/// The reply `n * 2`.
fn double() -> Value {
    let number = json!({"t": "Number"});
    json!({"body": {"kind": "binary", "op": "mul",
        "left": {"kind": "input", "name": "n"},
        "right": {"kind": "literal", "type": number, "value": 2}}})
}

/// Runs the loop for `Double` on `provider` with `options`.
fn run(provider: &dyn velme_synth::SynthProvider, options: SynthOptions) -> Outcome {
    let program = program(SOURCE);
    let id = goal_id(&program, "Double");
    let task = Task {
        program: &program,
        goal: id,
        source: SOURCE,
        contract_key: contract_key(&program, id).expect("a key"),
        synthesis_key: Fingerprint::of_bytes(b"synthesis"),
        feedback: Vec::new(),
    };
    let mut session = Session::new(options);
    block_on(synthesize(&mut session, provider, &task, &LeafRunner::new()))
}

fn ollama(server: &MockServer) -> (Box<dyn velme_synth::SynthProvider>, RecordingSleeper) {
    let sleeper = RecordingSleeper::default();
    let mut config = OllamaConfig::new("m");
    config.url = Some(velme_synth::ExternalUrl::parse(server.url()).expect("a URL"));
    let backend = Ollama::new(config).with_sleeper(Arc::new(sleeper.clone()));
    let identity = block_on(backend.identify()).expect("identity");
    (backend.open(&identity).expect("a provider"), sleeper)
}

fn anthropic(server: &MockServer) -> (velme_synth::Anthropic, RecordingSleeper) {
    let sleeper = RecordingSleeper::default();
    let provider = mock_anthropic(AnthropicConfig::new("m"), server, KEY, &sleeper);
    (provider, sleeper)
}

fn chat_requests(server: &MockServer) -> Vec<Value> {
    server
        .requests()
        .iter()
        .filter(|r| r.path == "/api/chat")
        .map(|r| r.json())
        .collect()
}

fn tags() -> MockResponse {
    MockResponse::ollama_tags(&[("m:latest", DIGEST)])
}

/// An `ollama` request carries 2048 output tokens and an `anthropic` one 8192 unless `max_output_tokens` says otherwise
/// (AC-SYNTH-45).
#[test]
fn ac_synth_45_the_request_carries_the_provider_default_or_the_configured_limit() {
    let server = MockServer::start([tags(), MockResponse::ollama_chat(&double())]);
    let (provider, _) = ollama(&server);
    assert!(matches!(
        run(provider.as_ref(), SynthOptions::default()),
        Outcome::Built(_)
    ));
    assert_eq!(chat_requests(&server)[0]["options"]["num_predict"], 2048);

    let server = MockServer::start([tags(), MockResponse::ollama_chat(&double())]);
    let (provider, _) = ollama(&server);
    let options = SynthOptions {
        max_output_tokens: Some(300),
        ..SynthOptions::default()
    };
    assert!(matches!(run(provider.as_ref(), options), Outcome::Built(_)));
    assert_eq!(chat_requests(&server)[0]["options"]["num_predict"], 300);

    let server = MockServer::start([MockResponse::tool_call("write_goal", &double())]);
    let (provider, _) = anthropic(&server);
    assert!(matches!(run(&provider, SynthOptions::default()), Outcome::Built(_)));
    assert_eq!(server.requests()[0].json()["max_tokens"], 8192);

    let server = MockServer::start([MockResponse::tool_call("write_goal", &double())]);
    let (provider, _) = anthropic(&server);
    let options = SynthOptions {
        max_output_tokens: Some(4096),
        ..SynthOptions::default()
    };
    assert!(matches!(run(&provider, options), Outcome::Built(_)));
    assert_eq!(server.requests()[0].json()["max_tokens"], 4096);
}

/// A `Timeout` after the request was sent is not retried by either provider: one request, `VL0404` (AC-SYNTH-45, D-110).
#[test]
fn ac_synth_45_a_generation_that_times_out_is_not_retried() {
    let options = SynthOptions {
        timeout_secs: 1,
        ..SynthOptions::default()
    };
    let failed = |outcome: Outcome| match outcome {
        Outcome::Failed(failure) => failure.diagnostic.code,
        Outcome::Built(_) => panic!("it was built"),
    };

    let server = MockServer::start([tags(), MockResponse::hang(), MockResponse::hang()]);
    let (provider, sleeper) = ollama(&server);
    assert_eq!(
        failed(run(provider.as_ref(), options.clone())),
        Code::ProviderUnavailable
    );
    assert_eq!(chat_requests(&server).len(), 1);
    assert!(sleeper.waits().is_empty());

    let server = MockServer::start([MockResponse::hang(), MockResponse::hang()]);
    let (provider, sleeper) = anthropic(&server);
    assert_eq!(failed(run(&provider, options)), Code::ProviderUnavailable);
    assert_eq!(server.requests().len(), 1);
    assert!(sleeper.waits().is_empty());
}

/// A refused connection is still retried, twice, after the same waits (R-SYNTH-12).
#[test]
fn ac_synth_45_a_refused_connection_is_still_retried() {
    let server = MockServer::start([]);
    let (provider, sleeper) = anthropic(&server);
    drop(server);
    let outcome = run(&provider, SynthOptions::default());
    assert!(matches!(outcome, Outcome::Failed(_)));
    assert_eq!(sleeper.waits().len(), 2);
}
