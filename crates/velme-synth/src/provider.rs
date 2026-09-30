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
    /// The model id, Ollama's `<model>@<digest>` or an external backend's `<backend>@<backend_version>` (`model_version` in
    /// the manifest).
    pub model: String,
    /// The `prompt_version` plus request options, or an external backend's `request_version` (D-97).
    pub input_version: String,
    /// An external backend's own name, from its `describe` reply (`runtime/32` R-ART-21, R-SYNTH-26); none for any other
    /// provider.
    pub backend: Option<String>,
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

/// A source of candidate IR: an LLM, an external service, or a test double. One [`SynthProvider::complete`] call is
/// one provider call, whatever happens inside it (`compiler/22` R-SYNTH-21, D-92).
#[async_trait]
pub trait SynthProvider: Send + Sync {
    /// `anthropic`, `ollama`, `external`, `replay` or `scripted`.
    fn id(&self) -> &str;

    /// The model id (or `<model>+<retry_model>`, R-SYNTH-39), Ollama's digest, or an external `<backend>@<backend_version>`.
    fn model(&self) -> &str;

    /// The `prompt_version` plus request options (LLM providers, R-SYNTH-40), or the `request_version` (external).
    fn input_version(&self) -> &str;

    /// What diagnostics call the provider: an external backend's own name, else the provider id.
    fn backend(&self) -> &str {
        self.id()
    }

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
    /// Which attempt of the goal's loop this call is, from 0: what `retry_model` keys on (R-SYNTH-39). A request that
    /// carries earlier feedback (R-SYNTH-46) is still attempt 0, so the request itself is unchanged.
    pub attempt: u32,
}

impl SynthLimits {
    /// The defaults of `tooling/40` §5.1 for the goal with `synthesis_key`.
    pub fn new(synthesis_key: Fingerprint) -> Self {
        SynthLimits {
            timeout: Duration::from_secs(60),
            max_output_tokens: 8192,
            synthesis_key,
            attempt: 0,
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
    /// The provider rejected the API key that is set (`VL0405`, its own wording).
    KeyRejected,
    /// An external backend rejected the bearer token that is sent, with `401` or `403` (`VL0405`, its own wording).
    TokenRejected {
        /// Whether a token was sent; if not, the backend wants one.
        sent: bool,
    },
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
    /// An external backend failed (`VL0406`, R-SYNTH-28).
    BackendFailed {
        /// Why, in one cleaned line: Velme's wording, or the backend's own `{"error"}` text, collapsed and bounded.
        reason: String,
        /// The cleaned tail of the last 4 KiB of the backend's reply body, empty if there was none.
        body: String,
    },
    /// An external backend queued the request for later (`VL0408`, R-SYNTH-41). The text is untrusted.
    Pending(String),
    /// A replay fixture or `replay.json` couldn't be read or written, or is a link or another file that isn't regular
    /// (`VL0901`, R-SYNTH-43).
    File {
        /// The file.
        path: String,
        /// Why, in Velme's wording.
        reason: String,
    },
    /// A bug in Velme itself, such as a request that has no hash (`VL0607`); never a verdict on the candidate.
    Internal(String),
}

impl ProviderError {
    /// The variant's name as replay fixtures and scripts spell it (R-SYNTH-43): `refused`, `pending`, …
    pub fn variant(&self) -> &'static str {
        match self {
            ProviderError::NotConfigured => "not_configured",
            ProviderError::KeyRejected => "key_rejected",
            ProviderError::TokenRejected { sent: true } => "token_rejected",
            ProviderError::TokenRejected { sent: false } => "token_required",
            ProviderError::Unavailable(_) => "unavailable",
            ProviderError::RateLimited { .. } => "rate_limited",
            ProviderError::Refused(_) => "refused",
            ProviderError::Timeout => "timeout",
            ProviderError::Malformed(_) => "malformed",
            ProviderError::BackendFailed { .. } => "backend_failed",
            ProviderError::Pending(_) => "pending",
            ProviderError::File { .. } => "file",
            ProviderError::Internal(_) => "internal",
        }
    }

    /// The variant called `name`, or `None` for a name that is not one. Only `pending` keeps its text: the text of the
    /// others is provider prose that fixtures never hold (R-SYNTH-43), so they come back empty.
    pub fn from_variant(name: &str, text: Option<&str>) -> Option<Self> {
        Some(match name {
            "not_configured" => ProviderError::NotConfigured,
            "key_rejected" => ProviderError::KeyRejected,
            "token_rejected" => ProviderError::TokenRejected { sent: true },
            "token_required" => ProviderError::TokenRejected { sent: false },
            "unavailable" => ProviderError::Unavailable(String::new()),
            "rate_limited" => ProviderError::RateLimited { retry_after: None },
            "refused" => ProviderError::Refused(String::new()),
            "timeout" => ProviderError::Timeout,
            "malformed" => ProviderError::Malformed(String::new()),
            "backend_failed" => ProviderError::BackendFailed {
                reason: String::new(),
                body: String::new(),
            },
            "pending" => ProviderError::Pending(text.unwrap_or_default().to_owned()),
            "internal" => ProviderError::Internal(String::new()),
            _ => return None,
        })
    }
}
