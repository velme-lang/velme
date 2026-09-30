//! The provider interface (`compiler/22` §3.1, D-13, D-14, D-92): plain Rust and `serde_json`, with no vendor type in a
//! signature (R-SYNTH-04).

use std::time::Duration;

use async_trait::async_trait;
use velme_ir::Fingerprint;

use crate::request::SynthRequest;

/// What a provider reports once per build for `synthesis_key` (R-SYNTH-25): the provider id, the model and the input
/// version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `anthropic`, `ollama`, `external`, `replay` or `scripted`.
    pub provider: String,
    /// The model id, Ollama's `<model>@<digest>` or an external backend's `backend_version` (`model_version` in the
    /// manifest).
    pub model: String,
    /// The `prompt_version` plus request options, or an external backend's `request_version` (D-97).
    pub input_version: String,
}

/// A provider before it is built (D-92). The identity step contacts whatever the provider needs to know its identity
/// and runs once, on the first lock miss of a build; the provider that makes requests is built only when a goal also
/// misses the store. A fully cached build calls neither (R-SYNTH-21, R-SYNTH-25).
#[async_trait]
pub trait SynthBackend: Send + Sync {
    /// The identity step: an Ollama digest lookup, an external `describe`, a replay's `replay.json`; nothing for
    /// `anthropic` and `scripted`.
    async fn identify(&self) -> Result<Identity, ProviderError>;

    /// The provider that answers requests, for a backend whose identity is `identity`.
    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError>;
}

/// A source of candidate IR: an LLM, an external program, or a test double. One [`SynthProvider::complete`] call is
/// one provider call, whatever happens inside it (`compiler/22` R-SYNTH-21, D-92).
#[async_trait]
pub trait SynthProvider: Send + Sync {
    /// `anthropic`, `ollama`, `external`, `replay` or `scripted`.
    fn id(&self) -> &str;

    /// The model id (or `<model>+<retry_model>`, R-SYNTH-39), Ollama's digest, or an external `backend_version`.
    fn model(&self) -> &str;

    /// The `prompt_version` plus request options (LLM providers, R-SYNTH-40), or the `request_version` (external).
    fn input_version(&self) -> &str;

    /// Asks for one candidate for `request`.
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError>;
}

/// What bounds one call, and which goal it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthLimits {
    /// The most time one request may take (`timeout_secs`).
    pub timeout: Duration,
    /// The most tokens a reply may hold; LLM providers only (`max_output_tokens`).
    pub max_output_tokens: u32,
    /// The goal's `synthesis_key`, which names its replay fixture and its synth-log lines (R-SYNTH-23, R-SYNTH-43).
    pub synthesis_key: Fingerprint,
}

impl SynthLimits {
    /// The defaults of `tooling/40` §5.1 for the goal with `synthesis_key`.
    pub fn new(synthesis_key: Fingerprint) -> Self {
        SynthLimits {
            timeout: Duration::from_secs(60),
            max_output_tokens: 8192,
            synthesis_key,
        }
    }
}

/// A provider's answer: one reply document, exactly as received (R-SYNTH-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthReply {
    /// The reply document: an IR goal or a question, not yet read or validated.
    pub reply_json: String,
    /// Tokens used.
    pub usage: Usage,
    /// How long the exchange took.
    pub latency: Duration,
}

/// Token counts of one exchange; zero where a provider doesn't report them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens sent.
    pub input_tokens: u64,
    /// Tokens received.
    pub output_tokens: u64,
    /// Prompt-cache tokens read (R-SYNTH-34).
    pub cache_read_tokens: u64,
    /// Prompt-cache tokens written (R-SYNTH-34).
    pub cache_write_tokens: u64,
}

/// Why a provider gave no reply (`compiler/22` R-SYNTH-07 maps each to a diagnostic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// No provider, model or key is configured, or the model isn't there (`VL0405`).
    NotConfigured,
    /// Unreachable, or a transport failure that outlived its retries (`VL0404`).
    Unavailable(String),
    /// Rate limited, after the allowed waits (`VL0404`).
    RateLimited {
        /// How long the provider asked to wait.
        retry_after: Option<Duration>,
    },
    /// The provider declined to answer (a failed attempt, `VL0401`).
    Refused(String),
    /// The request outlived its time (`VL0404`).
    Timeout,
    /// The reply was not a reply (a failed attempt, `VL0401`).
    Malformed(String),
    /// An external backend failed (`VL0406`).
    BackendFailed(String),
    /// An external backend queued the request for later (`VL0408`, R-SYNTH-41). The text is untrusted.
    Pending(String),
}

impl ProviderError {
    /// The variant's name as replay fixtures and scripts spell it (R-SYNTH-43): `refused`, `pending`, …
    pub fn variant(&self) -> &'static str {
        match self {
            ProviderError::NotConfigured => "not_configured",
            ProviderError::Unavailable(_) => "unavailable",
            ProviderError::RateLimited { .. } => "rate_limited",
            ProviderError::Refused(_) => "refused",
            ProviderError::Timeout => "timeout",
            ProviderError::Malformed(_) => "malformed",
            ProviderError::BackendFailed(_) => "backend_failed",
            ProviderError::Pending(_) => "pending",
        }
    }

    /// The variant called `name`, or `None` for a name that is not one. Only `pending` keeps its text: the text of the
    /// others is provider prose that fixtures never hold (R-SYNTH-43), so they come back empty.
    pub fn from_variant(name: &str, text: Option<&str>) -> Option<Self> {
        Some(match name {
            "not_configured" => ProviderError::NotConfigured,
            "unavailable" => ProviderError::Unavailable(String::new()),
            "rate_limited" => ProviderError::RateLimited { retry_after: None },
            "refused" => ProviderError::Refused(String::new()),
            "timeout" => ProviderError::Timeout,
            "malformed" => ProviderError::Malformed(String::new()),
            "backend_failed" => ProviderError::BackendFailed(String::new()),
            "pending" => ProviderError::Pending(text.unwrap_or_default().to_owned()),
            _ => return None,
        })
    }
}
