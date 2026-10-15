//! Notation provider contracts and prompt budget (R-SYNTH-55..56).
#![allow(clippy::expect_used, clippy::indexing_slicing)]
#![cfg(all(
    feature = "provider-anthropic",
    feature = "provider-ollama",
    feature = "provider-external"
))]

use serde_json::{Value, json};
use velme_synth::{
    Anthropic, AnthropicConfig, External, ExternalConfig, ExternalUrl, Ollama, OllamaConfig, ReplyFormat,
    SchemaInPrompt, SynthBackend, SynthLimits, SynthOptions, build_request, render_with,
};
use velme_test_support::{goal_id, program};

const SOURCE: &str = include_str!("notation/prompt.velme");

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn anthropic(schema: SchemaInPrompt) -> Value {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("request");
    let mut config = AnthropicConfig::new("test");
    config.options = SynthOptions {
        reply_format: ReplyFormat::Notation,
        schema_in_prompt: schema,
        ..SynthOptions::default()
    };
    let text = Anthropic::new(config)
        .request_body(&request, &SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"test")))
        .expect("request body");
    serde_json::from_str(&text).expect("JSON")
}

#[test]
fn ac_synth_55_leaf_prompt_and_fixed_prefix_meet_the_byte_budgets() {
    let body = anthropic(SchemaInPrompt::Summary);
    assert!(body.to_string().len() <= 7_168, "{}", body.to_string().len());
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("request");
    let options = SynthOptions {
        reply_format: ReplyFormat::Notation,
        ..SynthOptions::default()
    };
    let prompt = render_with(&request, &options.prompt()).expect("prompt");
    let fixed = body["system"].to_string().len() + body["tools"].to_string().len() + prompt.prefix.len();
    assert!(fixed <= 5_120, "{fixed}");
    insta::assert_snapshot!(
        "notation_leaf_prompt",
        prompt
            .first_turn()
            .replace(&velme_synth::prompt_version(), "<prompt_version>")
    );
    let composite = build_request(&program, goal_id(&program, "Summary"), SOURCE).expect("composite");
    let prompt = render_with(&composite, &options.prompt()).expect("prompt");
    assert!(prompt.task.contains("each result below is a name the\nbody reads"));
    insta::assert_snapshot!(
        "notation_composite_prompt",
        prompt
            .first_turn()
            .replace(&velme_synth::prompt_version(), "<prompt_version>")
    );
}

#[test]
fn ac_synth_56_both_providers_constrain_only_a_text_body_or_question() {
    for schema in [SchemaInPrompt::Summary, SchemaInPrompt::Full] {
        let anthropic = anthropic(schema);
        assert_eq!(
            anthropic["tools"][0]["input_schema"]["properties"]["body"],
            json!({"type":"string"})
        );
        let mut config = OllamaConfig::new("test");
        config.options = SynthOptions {
            reply_format: ReplyFormat::Notation,
            schema_in_prompt: schema,
            ..SynthOptions::default()
        };
        let backend = Ollama::new(config);
        let program = program(SOURCE);
        let request = build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("request");
        let text = backend
            .request_body(&request, &SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"test")))
            .expect("request body");
        let ollama: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(
            ollama["format"]["anyOf"][0]["properties"]["body"],
            json!({"type":"string"})
        );
        for body in [anthropic, ollama] {
            let text = body.to_string();
            assert!(!text.contains("$defs"));
            for definition in velme_ir::schema()["$defs"].as_object().expect("definitions").keys() {
                assert!(!text.contains(&format!("\\\"{definition}\\\"")), "{definition}");
            }
        }
    }
}

#[test]
fn ac_synth_60_external_notation_requires_advertising_and_uses_the_grammar_version() {
    use velme_test_support::backend::{Config, Server};
    for supported in [false, true] {
        let server = Server::start(if supported {
            Config::default().listing(json!(["ir-json", "notation"]))
        } else {
            Config::default()
        });
        let mut config = ExternalConfig::new(ExternalUrl::parse(server.url()).expect("URL"));
        config.reply_format = ReplyFormat::Notation;
        let identity = block_on(External::new(config).identify());
        if supported {
            assert_eq!(identity.expect("identity").input_version, "0.1+reply_format=notation@1");
        } else {
            let error = identity.expect_err("unlisted");
            assert_eq!(
                velme_synth::provider_diagnostic(&error, "test", "FindBadge", Default::default()).code,
                velme_diagnostics::Code::BackendFailed
            );
        }
    }
}
