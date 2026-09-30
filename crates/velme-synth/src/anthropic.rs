//! The `anthropic` provider (`compiler/22` R-SYNTH-05, R-SYNTH-44, D-14, D-98): the Messages API at temperature 0, with
//! the reply returned through two forced tools, `write_goal` and `ask_question`. The key comes from the environment at
//! request time and lives in a wrapper that prints `***` (`tooling/41` R-SEC-05, R-SEC-06); no error, `Debug` output or
//! diagnostic this module makes holds it, or any text the provider sent back (R-SYNTH-22, D-93).

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use velme_ir::{MAX_JSON_DEPTH, from_json_str_within, to_canonical_string};

use crate::http::{agent, read_body, transport_error};
use crate::options::{ReplyFormat, SynthOptions};
use crate::prompt::{Role, prompt_version, render_with};
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::SynthRequest;
use crate::transport::{ENVELOPE_DEPTH, Sleeper, StdSleeper, with_transport_retries};

/// Where the Messages API is, unless a test says otherwise (D-98).
const BASE_URL: &str = "https://api.anthropic.com";

/// The API version header value.
const API_VERSION: &str = "2023-06-01";

/// The tool whose input is an IR goal, and the one whose input is a question (R-SYNTH-44).
const WRITE_GOAL: &str = "write_goal";
const ASK_QUESTION: &str = "ask_question";

/// An API key. Its `Debug` and `Display` print `***` (R-SEC-06); the text leaves it only into the request header.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// A key with the text `key`.
    pub fn new(key: impl Into<String>) -> Self {
        ApiKey(key.into())
    }

    /// The key in the environment (`tooling/40` §5.2): `VELME_API_KEY`, else `ANTHROPIC_API_KEY`, trimmed. A variable that is empty is not
    /// set. One that is set to something that can't be a key, anything but visible ASCII once trimmed, is an error
    /// naming it: it is not skipped for the next variable, so a typo in `VELME_API_KEY` never sends a request under
    /// another key.
    pub fn lookup() -> Result<Self, KeyError> {
        Self::pick(["VELME_API_KEY", "ANTHROPIC_API_KEY"].map(|name| {
            let value = match std::env::var(name) {
                Ok(value) => Some(value),
                // Set, but not text: it can't be a key.
                Err(std::env::VarError::NotUnicode(_)) => Some("\u{fffd}".to_owned()),
                Err(std::env::VarError::NotPresent) => None,
            };
            (name, value)
        }))
    }

    /// The first of `vars` that is set and not empty, as a key, or the error naming it.
    fn pick(vars: [(&'static str, Option<String>); 2]) -> Result<Self, KeyError> {
        for (name, value) in vars {
            let Some(value) = value else { continue };
            let key = ApiKey(value.trim().to_owned());
            if key.0.is_empty() {
                continue;
            }
            return if key.is_usable() {
                Ok(key)
            } else {
                Err(KeyError::Malformed(name))
            };
        }
        Err(KeyError::Missing)
    }

    /// Whether the text can be an `x-api-key` header value: non-empty visible ASCII.
    fn is_usable(&self) -> bool {
        !self.0.is_empty() && self.0.bytes().all(|b| b.is_ascii_graphic())
    }
}

/// Why the environment holds no key to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Neither variable is set to anything.
    Missing,
    /// This variable is set to something that can't be an API key.
    Malformed(&'static str),
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// The settings of the `anthropic` provider (`tooling/40` §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    /// The model id; there is no built-in default.
    pub model: String,
    /// The model for retries; attempt 0 uses `model` (R-SYNTH-39).
    pub retry_model: Option<String>,
    /// Whether the end of the fixed prompt prefix is a cache breakpoint (R-SYNTH-34).
    pub prompt_cache: bool,
    /// The settings that shape the prompt and enter `input_version` (R-SYNTH-40).
    pub options: SynthOptions,
}

impl AnthropicConfig {
    /// The defaults with `model`.
    pub fn new(model: impl Into<String>) -> Self {
        AnthropicConfig {
            model: model.into(),
            retry_model: None,
            prompt_cache: true,
            options: SynthOptions::default(),
        }
    }
}

/// Where the key is read from, each time a request is made.
#[derive(Clone)]
enum KeySource {
    Env,
    #[cfg_attr(not(feature = "test-endpoint"), allow(dead_code))]
    Fixed(ApiKey),
}

impl KeySource {
    fn get(&self) -> Option<ApiKey> {
        match self {
            KeySource::Env => ApiKey::lookup().ok(),
            KeySource::Fixed(key) => Some(key.clone()).filter(ApiKey::is_usable),
        }
    }
}

/// The `anthropic` backend and provider. Its identity step contacts nothing (R-SYNTH-25).
#[derive(Clone)]
pub struct Anthropic {
    config: AnthropicConfig,
    model: String,
    input_version: String,
    base_url: String,
    key: KeySource,
    sleeper: Arc<dyn Sleeper>,
}

impl fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Anthropic")
            .field("model", &self.model)
            .field("input_version", &self.input_version)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    /// The provider for `config`, talking to the real API with the key from the environment.
    pub fn new(config: AnthropicConfig) -> Self {
        let model = match &config.retry_model {
            Some(retry) => format!("{}+{retry}", config.model),
            None => config.model.clone(),
        };
        let input_version = config.options.input_version(&prompt_version());
        Anthropic {
            config,
            model,
            input_version,
            base_url: BASE_URL.to_owned(),
            key: KeySource::Env,
            sleeper: Arc::new(StdSleeper),
        }
    }

    /// Waits between transport retries on `sleeper` instead of the host's clock (D-95).
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Points the provider at `base_url` with a fixed `key`. For tests against a local mock server only: it exists
    /// only under the `test-endpoint` feature, which `velme-test-support` alone turns on, and it accepts a loopback
    /// address only; never a config key or an environment variable (R-SYNTH-44, D-98).
    #[cfg(feature = "test-endpoint")]
    #[doc(hidden)]
    #[must_use]
    pub fn with_test_endpoint(mut self, base_url: impl Into<String>, key: ApiKey) -> Self {
        let base_url = base_url.into();
        assert!(
            matches!(crate::http::host_of(&base_url), "127.0.0.1" | "localhost"),
            "the test endpoint must be a loopback address"
        );
        self.base_url = base_url;
        self.key = KeySource::Fixed(key);
        self
    }

    /// The model for attempt `attempt` of a goal: `model` for attempt 0, `retry_model` for the rest (R-SYNTH-39).
    fn model_for(&self, attempt: u32) -> &str {
        match (&self.config.retry_model, attempt) {
            (Some(retry), 1..) => retry,
            _ => &self.config.model,
        }
    }

    /// The request body: the conversation of R-SYNTH-11 and the two tools, one of which the model must call.
    fn body(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<String, ProviderError> {
        let prompt = render_with(request, &self.config.options.prompt())
            .map_err(|_| ProviderError::Internal("the prompt could not be rendered".to_owned()))?;
        let mut messages = Vec::new();
        let mut prefix = json!({"type": "text", "text": prompt.prefix});
        if self.config.prompt_cache
            && let Some(block) = prefix.as_object_mut()
        {
            // The end of the fixed prefix: everything up to here is the same for every goal (R-SYNTH-34).
            block.insert("cache_control".to_owned(), json!({"type": "ephemeral"}));
        }
        let first = [prefix, json!({"type": "text", "text": prompt.task})];
        messages.push(json!({"role": "user", "content": first}));
        for turn in &prompt.retries {
            let role = match turn.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            messages.push(json!({"role": role, "content": [{"type": "text", "text": turn.text}]}));
        }
        let body = json!({
            "model": self.model_for(limits.attempt),
            "max_tokens": limits.max_output_tokens,
            "temperature": 0,
            "messages": messages,
            "tools": tools(request, self.config.options.reply_format)?,
            "tool_choice": {"type": "any"},
        });
        to_canonical_string(&body).map_err(|_| ProviderError::Internal("the request could not be written".to_owned()))
    }

    /// One HTTP exchange: the response body of a success, or the error the status or the transport means.
    fn send(&self, key: &ApiKey, body: &str, timeout: Duration) -> Result<String, ProviderError> {
        // A mock server on this machine is never reached through a proxy from the test's environment.
        let agent = agent(timeout, matches!(self.key, KeySource::Fixed(_)));
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let mut response = agent
            .post(&url)
            .header("x-api-key", key.0.as_str())
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send(body)
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        match status {
            200..=299 => read_body(&mut response),
            401 | 403 => Err(ProviderError::KeyRejected),
            404 => Err(ProviderError::NotConfigured),
            429 => Err(ProviderError::RateLimited { retry_after }),
            300..=399 | 408 | 500..=599 => Err(ProviderError::Unavailable(format!(
                "the provider answered with status {status}"
            ))),
            _ => Err(ProviderError::Internal(format!(
                "the provider rejected the request with status {status}"
            ))),
        }
    }
}

/// The two tools of R-SYNTH-44. `write_goal` takes the IR goal schema, `ask_question` the question object; the reply
/// schema's shared definitions move under the goal's, where its `$ref`s resolve. A compact reply (R-SYNTH-36) is not
/// in the schema's names, so its tool is left unconstrained and the validator alone decides.
fn tools(request: &SynthRequest, format: ReplyFormat) -> Result<Value, ProviderError> {
    let bad = || ProviderError::Internal("the reply schema has no goal or question".to_owned());
    let mut defs: Map<String, Value> = request
        .output_schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(bad)?;
    let goal = defs.remove("IrGoal").ok_or_else(bad)?;
    let question = defs.remove("Question").ok_or_else(bad)?;
    let goal = match (format, goal) {
        (ReplyFormat::IrJson, Value::Object(mut goal)) => {
            goal.insert("$defs".to_owned(), Value::Object(defs));
            Value::Object(goal)
        }
        _ => json!({"type": "object"}),
    };
    Ok(json!([
        {
            "name": WRITE_GOAL,
            "description": "Return the finished goal as IR JSON.",
            "input_schema": goal,
        },
        {
            "name": ASK_QUESTION,
            "description": "Ask one question, only when the plan leaves open a choice that changes the result.",
            "input_schema": question,
        },
    ]))
}

/// The reply and the tokens of a successful response (R-SYNTH-44): the input of the one tool called. A refusal is
/// `Refused`; no tool call, a second one, an unknown tool or a cut-off reply is `Malformed`.
fn read_response(text: &str) -> Result<(String, Usage), ProviderError> {
    let malformed = |why: &str| ProviderError::Malformed(why.to_owned());
    // The envelope adds a few levels around a reply, which is itself held to the IR depth limit.
    let doc: Value = from_json_str_within(text, MAX_JSON_DEPTH + ENVELOPE_DEPTH)
        .map_err(|_| malformed("the response wasn't JSON, or nested too deeply or repeated a key"))?;
    match doc.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => return Err(ProviderError::Refused(String::new())),
        Some("max_tokens") => return Err(malformed("the reply was cut off")),
        _ => {}
    }
    let mut calls = doc
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
    let (Some(call), None) = (calls.next(), calls.next()) else {
        return Err(malformed("the response didn't call exactly one tool"));
    };
    let name = call.get("name").and_then(Value::as_str);
    let input = call.get("input").filter(|input| input.is_object());
    let (true, Some(input)) = (matches!(name, Some(WRITE_GOAL | ASK_QUESTION)), input) else {
        return Err(malformed("the tool call wasn't one of the two tools with an object"));
    };
    let reply = to_canonical_string(input).map_err(|_| malformed("the tool input wasn't JSON"))?;
    let count = |name: &str| {
        doc.pointer(&format!("/usage/{name}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let usage = Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_read_tokens: count("cache_read_input_tokens"),
        cache_write_tokens: count("cache_creation_input_tokens"),
    };
    Ok((reply, usage))
}

#[async_trait]
impl SynthBackend for Anthropic {
    /// Contacts nothing (R-SYNTH-25).
    async fn identify(&self) -> Result<Identity, ProviderError> {
        Ok(Identity {
            provider: "anthropic".to_owned(),
            model: self.model.clone(),
            input_version: self.input_version.clone(),
            backend: None,
        })
    }

    fn open(&self, _identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl SynthProvider for Anthropic {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn input_version(&self) -> &str {
        &self.input_version
    }

    /// One provider call (R-SYNTH-21): the transport tries inside it wait 1 s then 2 s (R-SYNTH-12, D-95).
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let key = self.key.get().ok_or(ProviderError::NotConfigured)?;
        let body = self.body(request, limits)?;
        let started = Instant::now();
        let text = with_transport_retries(self.sleeper.as_ref(), || {
            std::future::ready(self.send(&key, &body, limits.timeout))
        })
        .await?;
        let (reply_json, usage) = read_response(&text)?;
        Ok(SynthReply {
            reply_json,
            usage,
            latency: started.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiKey, KeyError};

    fn pick(velme: Option<&str>, anthropic: Option<&str>) -> Result<ApiKey, KeyError> {
        ApiKey::pick([
            ("VELME_API_KEY", velme.map(str::to_owned)),
            ("ANTHROPIC_API_KEY", anthropic.map(str::to_owned)),
        ])
    }

    /// A key is trimmed, and an empty or blank variable is not set (`VL0405`).
    #[test]
    fn a_key_is_trimmed_and_an_empty_variable_is_not_set() {
        assert_eq!(pick(Some("  sk-1\n"), None), Ok(ApiKey("sk-1".to_owned())));
        assert_eq!(pick(Some("  "), Some("sk-2")), Ok(ApiKey("sk-2".to_owned())));
        assert_eq!(pick(None, Some("sk-3")), Ok(ApiKey("sk-3".to_owned())));
        assert_eq!(pick(Some(""), None), Err(KeyError::Missing));
        assert_eq!(pick(None, None), Err(KeyError::Missing));
    }

    /// A variable set to what can't be a key is an error naming it; it never falls back to the next variable, so a typo
    /// in `VELME_API_KEY` doesn't send a request under `ANTHROPIC_API_KEY`.
    #[test]
    fn a_malformed_key_is_an_error_naming_the_variable_and_never_falls_back() {
        for bad in ["sk\u{7}1", "sk-\u{e9}", "sk 3", "sk-\nx", "\u{fffd}"] {
            assert_eq!(
                pick(Some(bad), Some("sk-good")),
                Err(KeyError::Malformed("VELME_API_KEY")),
                "{bad:?}"
            );
            assert_eq!(
                pick(None, Some(bad)),
                Err(KeyError::Malformed("ANTHROPIC_API_KEY")),
                "{bad:?}"
            );
        }
    }
}
