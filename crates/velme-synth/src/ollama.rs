//! The `ollama` provider (`compiler/22` R-SYNTH-05, R-SYNTH-24, D-41, D-98): a local model through Ollama's chat API at
//! temperature 0, without streaming, with the reply schema as `format`, and no key. Its identity step reads the model's
//! digest from `/api/tags`; no error or diagnostic this module makes holds an address or text the server sent
//! (R-SYNTH-22).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use velme_ir::{MAX_JSON_DEPTH, from_json_str_within, to_canonical_string};

use crate::http::{agent_config_trusting, read_body, transport_error};
use crate::options::{ReplyFormat, SynthOptions};
use crate::prompt::{Role, prompt_version, render_with};
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::SynthRequest;
use crate::transport::{ENVELOPE_DEPTH, Sleeper, StdSleeper, with_generation_retries, with_transport_retries};
use crate::url::{ExternalUrl, LocalResolver};

/// Where Ollama listens unless `ollama_url` says otherwise (`tooling/40` §5.1).
pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";

/// The default `max_output_tokens` of an Ollama chat: its replies are a body only (D-103, D-110).
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 2048;

/// The longest a digest text may be: a `sha256:` digest is 71 characters.
const MAX_DIGEST_CHARS: usize = 128;

/// The settings of the `ollama` provider (`tooling/40` §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaConfig {
    /// The model, as `name` or `name:tag`; there is no built-in default.
    pub model: String,
    /// The model for retries; attempt 0 uses `model` (R-SYNTH-39).
    pub retry_model: Option<String>,
    /// The server's base URL, checked; `None` is [`DEFAULT_URL`]. Whether it names this machine, in any spelling, decides
    /// that the server is reached directly and never through a proxy (D-111).
    pub url: Option<ExternalUrl>,
    /// The settings that shape the prompt and enter `input_version` (R-SYNTH-40).
    pub options: SynthOptions,
    /// The most time the digest lookup may take; a chat request has its own `timeout` (`SynthLimits`).
    pub timeout: Duration,
}

impl OllamaConfig {
    /// The defaults with `model`.
    pub fn new(model: impl Into<String>) -> Self {
        OllamaConfig {
            model: model.into(),
            retry_model: None,
            url: None,
            options: SynthOptions::default(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// `model` with a tag: a name without one is `<name>:latest` (R-SYNTH-24). A `:` before the last `/` belongs to a
/// registry host, not a tag.
pub fn normalize_model(model: &str) -> String {
    let name = model.rsplit('/').next().unwrap_or(model);
    if name.contains(':') {
        model.to_owned()
    } else {
        format!("{model}:latest")
    }
}

/// The `ollama` backend. Its identity step asks the server for the model's digest (R-SYNTH-24, R-SYNTH-25).
#[derive(Clone)]
pub struct Ollama {
    config: OllamaConfig,
    model: String,
    /// The normalized `retry_model`.
    retry_model: Option<String>,
    /// The model and the retry model as the user wrote them, for what is said about a missing one.
    shown: (String, Option<String>),
    input_version: String,
    sleeper: Arc<dyn Sleeper>,
}

impl std::fmt::Debug for Ollama {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ollama")
            .field("model", &self.model)
            .field("host", &self.config.url.as_ref().map(ExternalUrl::host))
            .finish_non_exhaustive()
    }
}

impl Ollama {
    /// The backend for `config`.
    pub fn new(config: OllamaConfig) -> Self {
        let input_version = config.options.input_version(&prompt_version());
        Ollama {
            model: normalize_model(config.model.trim()),
            retry_model: config.retry_model.as_deref().map(|m| normalize_model(m.trim())),
            shown: (
                config.model.trim().to_owned(),
                config.retry_model.as_deref().map(|m| m.trim().to_owned()),
            ),
            config,
            input_version,
            sleeper: Arc::new(StdSleeper),
        }
    }

    /// Waits between transport retries on `sleeper` instead of the host's clock (D-95).
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// The model for attempt `attempt` of a goal: `model` for attempt 0, `retry_model` for the rest (R-SYNTH-39).
    fn model_for(&self, attempt: u32) -> &str {
        match (&self.retry_model, attempt) {
            (Some(retry), 1..) => retry,
            _ => &self.model,
        }
    }

    /// The URL of the endpoint `path`, which starts with `/`.
    fn endpoint(&self, path: &str) -> String {
        match &self.config.url {
            Some(url) => url.endpoint(path),
            None => format!("{DEFAULT_URL}{path}"),
        }
    }

    /// The model for attempt `attempt` as the user wrote it.
    fn shown_for(&self, attempt: u32) -> &str {
        match (&self.shown.1, attempt) {
            (Some(retry), 1..) => retry,
            _ => &self.shown.0,
        }
    }

    /// Whether the server is on this machine, in any spelling of its address, and so reached without a proxy (D-111).
    fn reaches_directly(&self) -> bool {
        self.config.url.as_ref().is_none_or(ExternalUrl::is_loopback)
    }

    /// An agent that resolves `localhost` without DNS and reaches a server on this machine without a proxy (D-102, D-111).
    fn agent(&self, timeout: Duration) -> ureq::Agent {
        ureq::Agent::with_parts(
            agent_config_trusting(timeout, self.reaches_directly(), &[]),
            ureq::unversioned::transport::DefaultConnector::default(),
            LocalResolver,
        )
    }

    /// One `/api/tags` exchange: the body of a success.
    fn get_tags(&self) -> Result<String, ProviderError> {
        let mut response = self
            .agent(self.config.timeout)
            .get(&self.endpoint("/api/tags"))
            .call()
            .map_err(transport_error)?;
        match response.status().as_u16() {
            200..=299 => read_body(&mut response),
            429 => Err(ProviderError::RateLimited { retry_after: None }),
            status => Err(ProviderError::Unavailable(format!(
                "the server answered the model list with status {status}"
            ))),
        }
    }

    /// One `/api/chat` exchange: the body of a success.
    fn post_chat(&self, body: &str, timeout: Duration, attempt: u32) -> Result<String, ProviderError> {
        let mut response = self
            .agent(timeout)
            .post(&self.endpoint("/api/chat"))
            .header("content-type", "application/json")
            .send(body)
            .map_err(transport_error)?;
        match response.status().as_u16() {
            200..=299 => read_body(&mut response),
            // Ollama answers a model it doesn't have with 404.
            404 => Err(ProviderError::ModelMissing(self.shown_for(attempt).to_owned())),
            429 => Err(ProviderError::RateLimited { retry_after: None }),
            status @ (300..=399 | 408 | 500..=599) => Err(ProviderError::Unavailable(format!(
                "the server answered with status {status}"
            ))),
            status => Err(ProviderError::Internal(format!(
                "the server rejected the request with status {status}"
            ))),
        }
    }

    /// The request body: the conversation of R-SYNTH-11, the reply schema as `format`, temperature 0, no streaming.
    fn body(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<String, ProviderError> {
        let prompt = render_with(request, &self.config.options.prompt())
            .map_err(|_| ProviderError::Internal("the prompt could not be rendered".to_owned()))?;
        let mut messages = vec![json!({"role": "user", "content": prompt.first_turn()})];
        for turn in &prompt.retries {
            let role = match turn.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            messages.push(json!({"role": role, "content": turn.text}));
        }
        // A compact reply (R-SYNTH-36) is not in the schema's names, so it is constrained to JSON only and the validator
        // alone decides.
        let format = match self.config.options.reply_format {
            ReplyFormat::IrJson => request.output_schema.clone(),
            ReplyFormat::Compact => json!("json"),
        };
        let body = json!({
            "model": self.model_for(limits.attempt),
            "messages": messages,
            "stream": false,
            "format": format,
            "options": {"temperature": 0, "num_predict": limits.max_output_tokens},
        });
        to_canonical_string(&body).map_err(|_| ProviderError::Internal("the request could not be written".to_owned()))
    }
}

/// The digest of `model` in the body of `/api/tags` (R-SYNTH-24): kept whole. A model the server lacks is
/// `NotConfigured`; a list that isn't a model list, or a digest that can't be one, is a server Velme can't use.
fn find_digest(text: &str, model: &str) -> Result<String, ProviderError> {
    let unusable = |why: &str| ProviderError::Unavailable(why.to_owned());
    let doc: Value = from_json_str_within(text, MAX_JSON_DEPTH + ENVELOPE_DEPTH)
        .map_err(|_| unusable("the server's model list wasn't JSON"))?;
    let models = doc
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| unusable("the server's answer had no model list"))?;
    let entry = models.iter().find(|entry| {
        ["name", "model"]
            .iter()
            .filter_map(|field| entry.get(field).and_then(Value::as_str))
            .any(|name| normalize_model(name) == model)
    });
    let Some(entry) = entry else {
        return Err(ProviderError::NotConfigured);
    };
    let digest = entry.get("digest").and_then(Value::as_str).unwrap_or_default();
    let usable = !digest.is_empty()
        && digest.chars().count() <= MAX_DIGEST_CHARS
        && digest.chars().all(|c| c.is_ascii_alphanumeric() || c == ':');
    if usable {
        Ok(digest.to_owned())
    } else {
        Err(unusable("the server's model list had no usable digest"))
    }
}

/// The reply and the tokens of a chat response: the message's content, which is itself the reply document.
fn read_response(text: &str) -> Result<(String, Usage), ProviderError> {
    let malformed = |why: &str| ProviderError::Malformed(why.to_owned());
    let doc: Value = from_json_str_within(text, MAX_JSON_DEPTH + ENVELOPE_DEPTH)
        .map_err(|_| malformed("the response wasn't JSON, or nested too deeply or repeated a key"))?;
    if doc.get("done_reason").and_then(Value::as_str) == Some("length") {
        return Err(malformed("the reply was cut off"));
    }
    let content = doc
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("the response had no message"))?;
    let reply: Value = from_json_str_within(content, MAX_JSON_DEPTH)
        .map_err(|_| malformed("the reply wasn't JSON, or nested too deeply or repeated a key"))?;
    if !reply.is_object() {
        return Err(malformed("the reply wasn't an object"));
    }
    let reply = to_canonical_string(&reply).map_err(|_| malformed("the reply wasn't JSON"))?;
    let count = |name: &str| doc.get(name).and_then(Value::as_u64).unwrap_or(0);
    let usage = Usage {
        input_tokens: count("prompt_eval_count"),
        output_tokens: count("eval_count"),
        ..Usage::default()
    };
    Ok((reply, usage))
}

#[async_trait]
impl SynthBackend for Ollama {
    /// Resolves the model's digest from `/api/tags` (R-SYNTH-24): `<model>@<digest>`, a model with no tag as
    /// `<name>:latest`; with a `retry_model`, `<model>@<digest>+<retry_model>@<digest>` (R-SYNTH-39). The transport tries
    /// inside wait as the providers' do (R-SYNTH-12).
    async fn identify(&self) -> Result<Identity, ProviderError> {
        let text = with_transport_retries(self.sleeper.as_ref(), || std::future::ready(self.get_tags())).await?;
        // The model that is missing is the one named, the retry model as much as the primary (R-SYNTH-24, R-SYNTH-39).
        let missing = |error, shown: &str| match error {
            ProviderError::NotConfigured => ProviderError::ModelMissing(shown.to_owned()),
            other => other,
        };
        let digest = find_digest(&text, &self.model).map_err(|e| missing(e, self.shown_for(0)))?;
        let mut model = format!("{}@{digest}", self.model);
        if let Some(retry) = &self.retry_model {
            let digest = find_digest(&text, retry).map_err(|e| missing(e, self.shown_for(1)))?;
            model.push_str(&format!("+{retry}@{digest}"));
        }
        Ok(Identity {
            provider: "ollama".to_owned(),
            model,
            input_version: self.input_version.clone(),
            backend: None,
        })
    }

    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(OllamaProvider {
            server: self.clone(),
            identity: identity.clone(),
        }))
    }
}

/// The provider an [`Ollama`] opens, for the digest its identity step found.
struct OllamaProvider {
    server: Ollama,
    identity: Identity,
}

#[async_trait]
impl SynthProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn default_max_output_tokens(&self) -> u32 {
        DEFAULT_MAX_OUTPUT_TOKENS
    }

    fn model(&self) -> &str {
        &self.identity.model
    }

    fn input_version(&self) -> &str {
        &self.identity.input_version
    }

    /// One provider call (R-SYNTH-21): the transport tries inside it wait 1 s then 2 s (R-SYNTH-12, D-95).
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let body = self.server.body(request, limits)?;
        let started = Instant::now();
        let text = with_generation_retries(self.server.sleeper.as_ref(), || {
            std::future::ready(self.server.post_chat(&body, limits.timeout, limits.attempt))
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
    use super::{Ollama, OllamaConfig, normalize_model};
    use crate::url::ExternalUrl;

    /// A server on this machine is reached without a proxy whatever the spelling of its address, and no other is: the
    /// decision is the checked URL's, not a comparison of strings (D-111).
    #[test]
    fn every_spelling_of_this_machine_is_reached_directly() {
        let direct = |url: Option<&str>| {
            let mut config = OllamaConfig::new("m");
            config.url = url.map(|u| ExternalUrl::parse(u).expect("a URL"));
            Ollama::new(config).reaches_directly()
        };
        assert!(direct(None));
        for local in [
            "http://127.0.0.1:11434",
            "http://127.0.0.2:11434",
            "http://Localhost:11434",
            "http://LOCALHOST",
            "http://[::1]:11434",
            "http://[0:0:0:0:0:0:0:1]:11434",
        ] {
            assert!(direct(Some(local)), "{local}");
        }
        for remote in ["https://ollama.example.com", "https://127.0.0.1.evil.example"] {
            assert!(!direct(Some(remote)), "{remote}");
        }
    }

    /// A name with no tag gets `:latest`; a registry port is not a tag (R-SYNTH-24).
    #[test]
    fn a_tagless_model_gets_latest() {
        assert_eq!(normalize_model("llama3.1"), "llama3.1:latest");
        assert_eq!(normalize_model("llama3.1:8b"), "llama3.1:8b");
        assert_eq!(
            normalize_model("host:5000/library/llama3"),
            "host:5000/library/llama3:latest"
        );
        assert_eq!(normalize_model("host:5000/llama3:q4"), "host:5000/llama3:q4");
    }
}
