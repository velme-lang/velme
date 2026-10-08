//! `cargo xtask render-prompt` (P0c, dev tooling): its output is byte for byte the body the provider sends, with no API
//! key in the environment and no network. The providers run against a mock server and the tool runs as a process.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use velme_synth::{
    AnthropicConfig, Ollama, OllamaConfig, ReplyFormat, SchemaInPrompt, SynthBackend, SynthLimits, SynthProvider,
    build_request,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{RecordingSleeper, goal_id, mock_anthropic, program};

/// The tool itself, included so the test needs no second build of the crate (a nested `cargo build` with other features
/// rebuilds it every time).
#[allow(dead_code)] // its `main` is for the process
#[path = "../examples/render_prompt.rs"]
mod tool;

const SOURCE: &str = "language: velme/0.1

type Player:
    name: Text
    score: Number

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise.
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

/// What the tool prints for the goal: the very code the tool runs, which reads no key variable and opens no connection.
fn rendered(test: &str, extra: &[&str]) -> String {
    let mut args = vec![source(test, SOURCE), "FindBadge".to_owned()];
    args.extend(extra.iter().map(|a| (*a).to_owned()));
    tool::run(&args).expect("the tool renders")
}

/// A source file of its own for `test` (the tests run in parallel), and its path.
fn source(test: &str, text: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-prompt");
    fs::create_dir_all(&dir).expect("directory");
    let file = dir.join(format!("{test}.velme"));
    fs::write(&file, text).expect("source");
    file.to_string_lossy().into_owned()
}

fn limits() -> SynthLimits {
    SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"render-prompt"))
}

/// The body the `anthropic` provider sends for the goal under `config`.
fn anthropic_sent(config: AnthropicConfig) -> String {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("a request");
    let server = MockServer::start([]);
    let sleeper = RecordingSleeper::default();
    let provider = mock_anthropic(config, &server, "sk-ant-SENTINEL-KEY-0123456789", &sleeper);
    let _ = block_on(provider.complete(&request, &limits()));
    server.requests().first().expect("a request was sent").body.clone()
}

/// The body the `ollama` provider sends for the goal under `config`.
fn ollama_sent(config: OllamaConfig, max_output_tokens: u32) -> String {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("a request");
    let server = MockServer::start([MockResponse::ollama_tags(&[("m:latest", "sha256:d")])]);
    let mut config = config;
    config.url = Some(velme_synth::ExternalUrl::parse(server.url()).expect("a URL"));
    let backend = Ollama::new(config).with_sleeper(Arc::new(RecordingSleeper::default()));
    let identity = block_on(backend.identify()).expect("identity");
    let provider = backend.open(&identity).expect("a provider");
    let mut limits = limits();
    limits.max_output_tokens = max_output_tokens;
    let _ = block_on(provider.complete(&request, &limits));
    server.requests().get(1).expect("a chat request was sent").body.clone()
}

/// The `anthropic` body, defaults and then non-default options, equals what the provider sends.
#[test]
fn p0c_render_prompt_equals_the_anthropic_request_body() {
    let default = AnthropicConfig::new("stand-in");
    assert_eq!(
        rendered("anthropic-default", &["--provider", "anthropic"]),
        anthropic_sent(default)
    );

    let mut config = AnthropicConfig::new("claude-x");
    config.prompt_cache = false;
    config.options.reply_format = ReplyFormat::Compact;
    config.options.schema_in_prompt = SchemaInPrompt::Full;
    config.options.max_prompt_examples = 0;
    let flags = [
        "--model",
        "claude-x",
        "--no-prompt-cache",
        "--reply-format",
        "compact",
        "--schema-in-prompt",
        "full",
        "--max-examples",
        "0",
    ];
    assert_eq!(rendered("anthropic-options", &flags), anthropic_sent(config));
}

/// The `ollama` body, defaults and then non-default options, equals what the provider sends.
#[test]
fn p0c_render_prompt_equals_the_ollama_request_body() {
    let default = OllamaConfig::new("m");
    assert_eq!(
        rendered("ollama-default", &["--provider", "ollama", "--model", "m"]),
        ollama_sent(default, velme_synth::OLLAMA_DEFAULT_MAX_OUTPUT_TOKENS)
    );

    let mut config = OllamaConfig::new("m");
    config.options.reply_format = ReplyFormat::Compact;
    config.options.max_output_tokens = Some(1234);
    let flags = [
        "--provider",
        "ollama",
        "--model",
        "m",
        "--reply-format",
        "compact",
        "--max-output-tokens",
        "1234",
    ];
    assert_eq!(rendered("ollama-options", &flags), ollama_sent(config, 1234));
}

/// A goal that does not exist, and a flag that is unknown, repeated or missing its value, are errors, never ignored.
#[test]
fn p0c_render_prompt_refuses_a_missing_goal_and_a_bad_flag() {
    let file = source("missing-goal", SOURCE);
    let run = |extra: &[&str]| {
        let mut args = vec![file.clone(), "Nope".to_owned()];
        args.extend(extra.iter().map(|a| (*a).to_owned()));
        tool::run(&args)
    };
    assert!(run(&[]).is_err_and(|e| e.contains("no goal Nope")));
    let real = |extra: &[&str]| {
        let mut args = vec![file.clone(), "FindBadge".to_owned()];
        args.extend(extra.iter().map(|a| (*a).to_owned()));
        tool::run(&args)
    };
    for bad in [
        &["--attempt", "1"][..],
        &["--retry-model", "m"],
        &["--model"],
        &["--model", "--pretty"],
        &["--pretty", "--pretty"],
        &["stray"],
    ] {
        assert!(real(bad).is_err(), "{bad:?}");
    }
}

/// The `external` body is the canonical request itself.
#[test]
fn p0c_render_prompt_external_is_the_canonical_request() {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("a request");
    let expected = velme_ir::to_canonical_string(&request).expect("canonical");
    assert_eq!(rendered("external", &["--provider", "external"]), expected);
}

/// Under `compact` the `external` body names the format, as the provider's request does (D-156).
#[test]
fn p0c_render_prompt_external_names_a_non_default_format() {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE)
        .expect("a request")
        .for_external(ReplyFormat::Compact);
    let expected = velme_ir::to_canonical_string(&request).expect("canonical");
    assert_eq!(
        rendered(
            "external-compact",
            &["--provider", "external", "--reply-format", "compact"]
        ),
        expected
    );
}
