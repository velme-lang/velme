//! Dev tooling behind `cargo xtask render-prompt`: prints the exact request body a provider would send for one goal of a
//! source file, with no network, no API key and no `describe` call, so a coding agent can answer it as the model would
//! (`tools/agent-bridge/README.md`). The bodies come from the providers' own builders, never from a copy of their templates.
//! Not part of the product: nothing here is reachable from `velme`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;
use std::time::Duration;

use serde_json::Value;
use velme_ir::{Fingerprint, to_canonical_string};
use velme_sema::hir::GoalId;
use velme_sema::{SourceFile, analyze};
use velme_synth::{
    Anthropic, AnthropicConfig, DEFAULT_MAX_OUTPUT_TOKENS, OLLAMA_DEFAULT_MAX_OUTPUT_TOKENS, Ollama, OllamaConfig,
    ReplyFormat, SchemaInPrompt, SynthLimits, SynthOptions, build_request,
};

const USAGE: &str = "usage: render-prompt <file.velme> <Goal> [--provider anthropic|ollama|external] [--model ID] \
[--reply-format ir-json|compact|notation] [--schema-in-prompt summary|full] [--max-examples N] [--max-output-tokens N] \
[--no-prompt-cache] [--pretty]  (the first attempt's request only)";

/// The program; `pub` so the tests, which include this file, use the same code the tool runs.
pub fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("render-prompt: {message}");
            ExitCode::from(2)
        }
    }
}

/// The flags that take a value, and those that do not.
const VALUED: &[&str] = &[
    "--provider",
    "--model",
    "--reply-format",
    "--schema-in-prompt",
    "--max-examples",
    "--max-output-tokens",
];
const PLAIN: &[&str] = &["--no-prompt-cache", "--pretty"];

/// The command line after the two positionals, as flag and value; a flag that is not listed, a value that is another
/// flag, a missing value and a repeated flag are errors, never ignored.
fn flags(args: &[String]) -> Result<Vec<(&str, Option<&str>)>, String> {
    let mut found: Vec<(&str, Option<&str>)> = Vec::new();
    let mut rest = args.iter().map(String::as_str);
    while let Some(flag) = rest.next() {
        let value = if VALUED.contains(&flag) {
            match rest.next() {
                Some(value) if !value.starts_with("--") => Some(value),
                _ => return Err(format!("{flag} needs a value")),
            }
        } else if PLAIN.contains(&flag) {
            None
        } else {
            return Err(format!("unknown argument {flag}\n{USAGE}"));
        };
        if found.iter().any(|(seen, _)| *seen == flag) {
            return Err(format!("{flag} is given twice"));
        }
        found.push((flag, value));
    }
    Ok(found)
}

fn value<'a>(flags: &[(&str, Option<&'a str>)], flag: &str) -> Option<&'a str> {
    flags.iter().find(|(f, _)| *f == flag).and_then(|(_, v)| *v)
}

/// A flag whose value is one of the kebab-case names of a settings type.
fn named<T: serde::de::DeserializeOwned>(flags: &[(&str, Option<&str>)], flag: &str) -> Result<Option<T>, String> {
    match value(flags, flag) {
        None => Ok(None),
        Some(name) => serde_json::from_value(Value::String(name.to_owned()))
            .map(Some)
            .map_err(|_| format!("{flag} {name} isn't a known value")),
    }
}

fn number<T: std::str::FromStr>(flags: &[(&str, Option<&str>)], flag: &str) -> Result<Option<T>, String> {
    match value(flags, flag) {
        None => Ok(None),
        Some(text) => text.parse().map(Some).map_err(|_| format!("{flag} needs a number")),
    }
}

/// The text to print for the command line `args` (without the program name).
pub fn run(args: &[String]) -> Result<String, String> {
    let (Some(path), Some(goal)) = (args.first(), args.get(1)) else {
        return Err(USAGE.to_owned());
    };
    if path.starts_with("--") || goal.starts_with("--") {
        return Err(USAGE.to_owned());
    }
    let flags = flags(args.get(2..).unwrap_or_default())?;
    let has = |flag: &str| flags.iter().any(|(f, _)| *f == flag);
    let provider = value(&flags, "--provider").unwrap_or("anthropic");
    let mut options = SynthOptions::default();
    if let Some(format) = named::<ReplyFormat>(&flags, "--reply-format")? {
        options.reply_format = format;
    }
    match value(&flags, "--schema-in-prompt") {
        None => {}
        Some("summary") => options.schema_in_prompt = SchemaInPrompt::Summary,
        Some("full") => options.schema_in_prompt = SchemaInPrompt::Full,
        Some(other) => return Err(format!("--schema-in-prompt {other} isn't summary or full")),
    }
    if let Some(n) = number(&flags, "--max-examples")? {
        options.max_prompt_examples = n;
    }
    options.max_output_tokens = number(&flags, "--max-output-tokens")?;

    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let (program, diagnostics) = analyze(&SourceFile::new(path, &text));
    let program = program
        .filter(|_| !diagnostics.iter().any(|d| d.is_error()))
        .ok_or_else(|| format!("{path} has errors; run `velme check` on it first"))?;
    let at = program
        .goals
        .iter()
        .position(|g| g.name == *goal)
        .ok_or_else(|| format!("no goal {goal} in {path}"))?;
    let request = build_request(&program, GoalId(at), &text).map_err(|d| format!("no request: {}", d.message))?;

    let mut limits = SynthLimits::new(Fingerprint::of_bytes(b"render-prompt"));
    limits.timeout = Duration::from_secs(options.timeout_secs);
    let model = value(&flags, "--model").unwrap_or("stand-in").to_owned();
    let built = match provider {
        "anthropic" => {
            limits.max_output_tokens = options.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
            let mut config = AnthropicConfig::new(model);
            config.prompt_cache = !has("--no-prompt-cache");
            config.options = options;
            Anthropic::new(config).request_body(&request, &limits)
        }
        "ollama" => {
            limits.max_output_tokens = options.max_output_tokens.unwrap_or(OLLAMA_DEFAULT_MAX_OUTPUT_TOKENS);
            let mut config = OllamaConfig::new(model);
            config.options = options;
            Ollama::new(config).request_body(&request, &limits)
        }
        // The service gets the canonical request itself and no prompt (`compiler/22` §3.2).
        "external" => to_canonical_string(&request.for_external(options.reply_format))
            .map_err(|_| velme_synth::ProviderError::Internal("the request could not be written".to_owned())),
        other => return Err(format!("--provider {other} isn't anthropic, ollama or external")),
    };
    let body = built.map_err(|e| format!("the request could not be built: {e:?}"))?;
    if has("--pretty") {
        let parsed: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        return serde_json::to_string_pretty(&parsed).map_err(|e| e.to_string());
    }
    Ok(body)
}
