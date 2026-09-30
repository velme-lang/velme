//! The `anthropic` provider against a mock server (`compiler/22` R-SYNTH-05, R-SYNTH-12, R-SYNTH-34, R-SYNTH-39,
//! R-SYNTH-44, D-98): no network, and no wall-clock wait except the one timeout test.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fmt::Write;
use std::future::Future;
use std::time::Duration;

use serde_json::{Value, json};
use velme_ir::{Synthesis, contract_key, synthesis_key};
use velme_synth::{
    Anthropic, AnthropicConfig, ApiKey, ProviderError, ReplyFormat, SynthBackend, SynthLimits, SynthProvider,
    SynthReply, SynthRequest, build_request, prompt_version, render_with,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{RecordingSleeper, goal_id, mock_anthropic, program};

const SOURCE: &str = "language: velme/0.1

goal Rank(score: Number) -> Number:
    plan: \"Return the score. SECRET-PLAN-TEXT\"

goal Grade(mark: Number) -> Number:
    plan: \"Return the mark plus one.\"
";

const KEY: &str = "sk-ant-SENTINEL-KEY-0123456789";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
}

fn requests() -> (SynthRequest, SynthRequest) {
    let program = program(SOURCE);
    let of = |name| build_request(&program, goal_id(&program, name), SOURCE).expect("a request");
    (of("Rank"), of("Grade"))
}

fn limits() -> SynthLimits {
    SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"goal"))
}

fn goal_reply() -> Value {
    json!({"goal": "Rank", "note": 1.5})
}

/// A provider on a mock that answers with `responses`, and the mock and the sleeper to inspect afterwards.
fn mock(
    config: AnthropicConfig,
    responses: impl IntoIterator<Item = MockResponse>,
) -> (Anthropic, MockServer, RecordingSleeper) {
    let server = MockServer::start(responses);
    let sleeper = RecordingSleeper::default();
    let provider = mock_anthropic(config, &server, KEY, &sleeper);
    (provider, server, sleeper)
}

fn complete(provider: &Anthropic, request: &SynthRequest) -> Result<SynthReply, ProviderError> {
    block_on(provider.complete(request, &limits()))
}

/// The request sets temperature 0, forces one of the two tools, carries the goal schema and the question object as
/// their inputs, and the tool's input comes back as the reply, with usage (R-SYNTH-05, R-SYNTH-44).
#[test]
fn a_request_forces_one_of_two_tools_and_the_tool_input_is_the_reply() {
    let (provider, server, sleeper) = mock(
        AnthropicConfig::new("claude-test"),
        [MockResponse::tool_call("write_goal", &goal_reply())],
    );
    let (request, _) = requests();
    let reply = complete(&provider, &request).expect("a reply");
    assert_eq!(reply.reply_json, r#"{"goal":"Rank","note":1.5}"#);
    assert_eq!(
        (
            reply.usage.input_tokens,
            reply.usage.output_tokens,
            reply.usage.cache_read_tokens,
            reply.usage.cache_write_tokens
        ),
        (11, 7, 5, 3)
    );
    assert!(sleeper.waits().is_empty());
    let sent = server.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].path, "/v1/messages");
    assert_eq!(sent[0].headers["x-api-key"], KEY);
    assert_eq!(sent[0].headers["anthropic-version"], "2023-06-01");
    let body = sent[0].json();
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["max_tokens"], 8192);
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    let tools = body["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["name"], "write_goal");
    assert_eq!(tools[1]["name"], "ask_question");
    let goal = &tools[0]["input_schema"];
    assert!(
        goal["$defs"].get("Node").is_some(),
        "the IR definitions are beside the goal"
    );
    assert!(goal["$defs"].get("IrGoal").is_none() && goal["$defs"].get("Question").is_none());
    assert_eq!(tools[1]["input_schema"]["required"], json!(["question"]));
    assert_eq!(body["messages"].as_array().expect("messages").len(), 1);
}

/// `ask_question`'s input is the question object, returned as the reply (R-SYNTH-44).
#[test]
fn a_question_tool_call_is_the_reply() {
    let (provider, _server, _) = mock(
        AnthropicConfig::new("m"),
        [MockResponse::tool_call(
            "ask_question",
            &json!({"question": "Which way?"}),
        )],
    );
    let reply = complete(&provider, &requests().0).expect("a reply");
    assert_eq!(reply.reply_json, r#"{"question":"Which way?"}"#);
}

fn message(stop_reason: &str, content: &Value) -> MockResponse {
    MockResponse::ok(json!({"stop_reason": stop_reason, "content": content, "usage": {}}).to_string())
}

/// A refusal is `Refused`; no tool call, both tools, an unknown tool, a cut-off reply or a body that is not JSON is
/// `Malformed`, and neither carries the provider's text (R-SYNTH-44, D-93).
#[test]
fn a_refusal_or_a_reply_that_is_not_one_tool_call_is_a_failed_attempt() {
    let call = |name: &str| json!({"type": "tool_use", "id": "t", "name": name, "input": {"x": 1}});
    let (provider, _server, _) = mock(
        AnthropicConfig::new("m"),
        [
            message("refusal", &json!([{"type": "text", "text": "I won't, PROVIDER-WORDS"}])),
            message("end_turn", &json!([{"type": "text", "text": "prose PROVIDER-WORDS"}])),
            message("tool_use", &json!([call("write_goal"), call("ask_question")])),
            message("tool_use", &json!([call("run_shell")])),
            message(
                "tool_use",
                &json!([{"type": "tool_use", "id": "t", "name": "write_goal", "input": "text"}]),
            ),
            message("max_tokens", &json!([call("write_goal")])),
            MockResponse::ok("<html>PROVIDER-WORDS"),
        ],
    );
    let request = requests().0;
    assert_eq!(
        complete(&provider, &request).expect_err("refused"),
        ProviderError::Refused(String::new())
    );
    for _ in 0..6 {
        let error = complete(&provider, &request).expect_err("malformed");
        assert!(matches!(&error, ProviderError::Malformed(_)), "{error:?}");
        assert!(!format!("{error:?}").contains("PROVIDER-WORDS"));
    }
}

/// A rejected key is `KeyRejected` and a missing model `NotConfigured` (both `VL0405`), a request Velme got wrong is
/// `Internal`, and the provider's body, which here echoes the key, reaches no error (R-SEC-06).
#[test]
fn status_codes_map_to_provider_errors_without_the_body() {
    let echo = |status| MockResponse::status(status, format!("{{\"error\": \"bad key {KEY}\"}}"));
    let (provider, _server, sleeper) = mock(
        AnthropicConfig::new("m"),
        [echo(401), echo(403), echo(404), echo(400), echo(413)],
    );
    let request = requests().0;
    for expected in [
        ProviderError::KeyRejected,
        ProviderError::KeyRejected,
        ProviderError::NotConfigured,
    ] {
        assert_eq!(complete(&provider, &request), Err(expected));
    }
    for _ in 0..2 {
        let error = complete(&provider, &request).expect_err("rejected");
        assert!(matches!(&error, ProviderError::Internal(_)), "{error:?}");
        assert!(!format!("{error:?}").contains(KEY));
    }
    assert!(sleeper.waits().is_empty(), "these are not transport failures");
}

/// A 429 waits its `retry_after` (at most 30 s) and a 5xx waits 1 s then 2 s, on the injected clock, inside one call
/// (R-SYNTH-12, D-95).
#[test]
fn rate_limits_and_server_errors_are_retried_on_the_injected_clock() {
    let ok = || MockResponse::tool_call("write_goal", &goal_reply());
    let (provider, server, sleeper) = mock(
        AnthropicConfig::new("m"),
        [
            MockResponse::status(429, "").header("retry-after", "5"),
            MockResponse::status(529, ""),
            ok(),
        ],
    );
    complete(&provider, &requests().0).expect("a reply after two waits");
    assert_eq!(sleeper.waits(), [Duration::from_secs(5), Duration::from_secs(2)]);
    assert_eq!(server.requests().len(), 3);

    let (provider, _server, sleeper) = mock(
        AnthropicConfig::new("m"),
        [
            MockResponse::status(429, "").header("retry-after", "99"),
            MockResponse::status(500, ""),
            MockResponse::status(502, ""),
        ],
    );
    let error = complete(&provider, &requests().0).expect_err("out of tries");
    assert!(matches!(error, ProviderError::Unavailable(_)), "{error:?}");
    assert_eq!(sleeper.waits(), [Duration::from_secs(30), Duration::from_secs(2)]);

    let (provider, _server, _) = mock(
        AnthropicConfig::new("m"),
        [
            MockResponse::status(429, ""),
            MockResponse::status(429, ""),
            MockResponse::status(429, ""),
        ],
    );
    assert_eq!(
        complete(&provider, &requests().0),
        Err(ProviderError::RateLimited { retry_after: None })
    );
}

/// A request that gets no answer within `timeout` is `Timeout`, after its transport retries (R-SYNTH-12).
#[test]
fn a_request_that_gets_no_answer_times_out() {
    let (provider, server, sleeper) = mock(
        AnthropicConfig::new("m"),
        [MockResponse::hang(), MockResponse::hang(), MockResponse::hang()],
    );
    let mut limits = limits();
    limits.timeout = Duration::from_millis(150);
    let result = block_on(provider.complete(&requests().0, &limits));
    assert_eq!(result, Err(ProviderError::Timeout));
    assert_eq!(server.requests().len(), 3);
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);
}

/// A server that isn't there is `Unavailable` with wording of Velme's own.
#[test]
fn an_unreachable_server_is_unavailable() {
    let server = MockServer::start([]);
    let sleeper = RecordingSleeper::default();
    let provider = mock_anthropic(AnthropicConfig::new("m"), &server, KEY, &sleeper);
    drop(server);
    let error = complete(&provider, &requests().0).expect_err("nothing there");
    assert!(matches!(&error, ProviderError::Unavailable(_)), "{error:?}");
    assert_eq!(sleeper.waits().len(), 2);
}

/// The two goals' prompts share a byte-identical prefix; the request marks a cache breakpoint at its end, and with
/// `prompt_cache = false` marks none. Both give the same identity, hence the same `synthesis_key` (AC-SYNTH-25).
#[test]
fn ac_synth_25_the_fixed_prefix_is_a_cache_breakpoint_and_caching_changes_no_key() {
    let (rank, grade) = requests();
    let options = AnthropicConfig::new("m").options.prompt();
    let prefix = |r: &SynthRequest| render_with(r, &options).expect("a prompt").prefix;
    assert_eq!(prefix(&rank), prefix(&grade));

    let cached = AnthropicConfig::new("m");
    let uncached = AnthropicConfig {
        prompt_cache: false,
        ..AnthropicConfig::new("m")
    };
    let ok = || MockResponse::tool_call("write_goal", &goal_reply());
    let (with_cache, server, _) = mock(cached.clone(), [ok(), ok()]);
    complete(&with_cache, &rank).expect("a reply");
    complete(&with_cache, &grade).expect("a reply");
    let (without_cache, plain, _) = mock(uncached.clone(), [ok()]);
    complete(&without_cache, &rank).expect("a reply");

    for sent in server.requests() {
        let content = sent.json()["messages"][0]["content"].clone();
        let blocks = content.as_array().expect("blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], prefix(&rank));
        assert_eq!(blocks[0]["cache_control"], json!({"type": "ephemeral"}));
        assert!(blocks[1].get("cache_control").is_none(), "only the prefix is cached");
    }
    let sent = plain.requests()[0].json();
    assert!(!sent.to_string().contains("cache_control"));

    let program = program(SOURCE);
    let contract = contract_key(&program, goal_id(&program, "Rank")).expect("a key");
    let key = |a: &Anthropic| {
        let identity = block_on(a.identify()).expect("identity");
        synthesis_key(
            contract,
            &Synthesis {
                input_version: &identity.input_version,
                compiler_version: "0.1.0",
                provider: &identity.provider,
                model: &identity.model,
            },
        )
        .expect("a key")
    };
    assert_eq!(key(&with_cache), key(&without_cache));
}

/// Attempt 0 uses `model` and retries use `retry_model`; `model()` is the pair. Changing `retry_model`, `reply_format`
/// or `max_prompt_examples` changes `synthesis_key` and never `contract_key` (AC-SYNTH-30).
#[test]
fn ac_synth_30_retries_use_the_retry_model_and_the_options_change_only_the_synthesis_key() {
    let config = AnthropicConfig {
        retry_model: Some("claude-retry".to_owned()),
        ..AnthropicConfig::new("claude-first")
    };
    let ok = || MockResponse::tool_call("write_goal", &goal_reply());
    let (provider, server, _) = mock(config.clone(), [ok(), ok(), ok()]);
    assert_eq!(provider.model(), "claude-first+claude-retry");
    let mut request = requests().0;
    complete(&provider, &request).expect("first attempt");
    request.attempts.push(velme_synth::AttemptFeedback {
        reply: "{}".to_owned(),
        diagnostics: Vec::new(),
    });
    let mut retry = limits();
    retry.attempt = 1;
    block_on(provider.complete(&request, &retry)).expect("a retry");
    let models: Vec<Value> = server.requests().iter().map(|r| r.json()["model"].clone()).collect();
    assert_eq!(models, ["claude-first", "claude-retry"]);
    // A first request that carries feedback from a re-verification (R-SYNTH-46) is attempt 0: `model`, not `retry_model`.
    complete(&provider, &request).expect("a re-verification");
    assert_eq!(server.requests()[2].json()["model"], "claude-first");
    // The retry turns are real conversation turns (D-95).
    let retry = server.requests()[1].json();
    assert_eq!(retry["messages"].as_array().expect("messages").len(), 3);
    assert_eq!(retry["messages"][1]["role"], "assistant");

    let program = program(SOURCE);
    let contract = contract_key(&program, goal_id(&program, "Rank")).expect("a key");
    let key = |config: &AnthropicConfig| {
        let identity = block_on(Anthropic::new(config.clone()).identify()).expect("identity");
        synthesis_key(
            contract,
            &Synthesis {
                input_version: &identity.input_version,
                compiler_version: "0.1.0",
                provider: &identity.provider,
                model: &identity.model,
            },
        )
        .expect("a key")
    };
    let base = key(&config);
    let mut other_retry = config.clone();
    other_retry.retry_model = Some("claude-other".to_owned());
    let mut compact = config.clone();
    compact.options.reply_format = ReplyFormat::Compact;
    let mut fewer = config.clone();
    fewer.options.max_prompt_examples = 2;
    for changed in [&other_retry, &compact, &fewer] {
        assert_ne!(key(changed), base);
    }
    assert_eq!(
        contract_key(&program, goal_id(&program, "Rank")).expect("a key"),
        contract
    );
    let identity = block_on(Anthropic::new(config).identify()).expect("identity");
    assert!(identity.input_version.starts_with(&prompt_version()));
}

/// A compact reply format leaves the goal tool unconstrained, as the schema's names are not the compact ones (R-SYNTH-36).
#[test]
fn a_compact_reply_leaves_the_goal_tool_unconstrained() {
    let mut config = AnthropicConfig::new("m");
    config.options.reply_format = ReplyFormat::Compact;
    let (provider, server, _) = mock(config, [MockResponse::tool_call("write_goal", &goal_reply())]);
    complete(&provider, &requests().0).expect("a reply");
    assert_eq!(
        server.requests()[0].json()["tools"][0]["input_schema"],
        json!({"type": "object"})
    );
}

/// The identity step contacts nothing (R-SYNTH-25).
#[test]
fn the_identity_step_contacts_nothing() {
    let (provider, server, _) = mock(AnthropicConfig::new("claude-test"), []);
    let identity = block_on(provider.identify()).expect("identity");
    assert_eq!(identity.provider, "anthropic");
    assert_eq!(identity.model, "claude-test");
    let opened = provider.open(&identity).expect("a provider");
    assert_eq!((opened.id(), opened.model()), ("anthropic", "claude-test"));
    assert!(server.requests().is_empty());
}

/// The key prints as `***` in `Debug` and `Display`, and appears in no error or debug text of the provider (R-SEC-06).
#[test]
fn ac_sec_05_the_key_is_redacted_everywhere_it_could_print() {
    let key = ApiKey::new(KEY);
    let mut shown = format!("{key:?} {key}");
    let (provider, _server, _) = mock(AnthropicConfig::new("m"), [MockResponse::status(401, KEY)]);
    write!(shown, " {provider:?}").expect("write");
    let error = complete(&provider, &requests().0).expect_err("rejected");
    write!(shown, " {error:?}").expect("write");
    assert!(!shown.contains(KEY) && !shown.contains("SENTINEL"), "{shown}");
    assert!(shown.contains("***"));
}

/// A goal nested as deep as the IR allows is accepted; a repeated key in the response is `Malformed`; so is a body over
/// the size limit, without a retry (R-SYNTH-09, R-IR-21).
#[test]
fn the_response_is_read_with_the_strict_bounded_parser() {
    let deep = |levels: usize| format!("{}1{}", r#"{"a":"#.repeat(levels), "}".repeat(levels));
    let envelope = |input: &str| {
        MockResponse::ok(format!(
            r#"{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"write_goal","input":{input}}}],"usage":{{}}}}"#
        ))
    };
    let (provider, _server, _) = mock(
        AnthropicConfig::new("m"),
        [envelope(&deep(500)), envelope(r#"{"a":1,"a":2}"#), envelope(&deep(700))],
    );
    let request = requests().0;
    assert_eq!(
        complete(&provider, &request).expect("a deep goal").reply_json,
        deep(500)
    );
    for _ in 0..2 {
        let error = complete(&provider, &request).expect_err("rejected");
        assert!(matches!(&error, ProviderError::Malformed(_)), "{error:?}");
    }

    let (provider, server, sleeper) = mock(
        AnthropicConfig::new("m"),
        [MockResponse::ok(" ".repeat(9 * 1024 * 1024))],
    );
    let error = complete(&provider, &request).expect_err("too large");
    assert!(matches!(&error, ProviderError::Malformed(_)), "{error:?}");
    assert_eq!((server.requests().len(), sleeper.waits().len()), (1, 0));
}

/// A key that can't be a header value is `NotConfigured` (`VL0405`) before any request, and is not retried.
#[test]
fn a_key_with_a_control_character_is_not_configured() {
    let server = MockServer::start([]);
    let sleeper = RecordingSleeper::default();
    let provider = mock_anthropic(AnthropicConfig::new("m"), &server, "sk-bad\nkey", &sleeper);
    assert_eq!(complete(&provider, &requests().0), Err(ProviderError::NotConfigured));
    assert!(server.requests().is_empty() && sleeper.waits().is_empty());
}
