//! The provider interface (`compiler/22` §3.1, D-13, D-14, D-92): plain Rust and `serde_json`, with no vendor type in a
//! signature (R-SYNTH-04).

use std::time::{Duration, Instant};

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

/// A source of candidate goal bodies: an LLM, an external service, or a test double. One [`SynthProvider::complete`] call is
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

    /// The `max_output_tokens` of a request unless `[synthesis]` sets one: 8192, and 2048 for `ollama`, whose replies are a
    /// body only (`tooling/40` §5.1, D-110).
    fn default_max_output_tokens(&self) -> u32 {
        DEFAULT_MAX_OUTPUT_TOKENS
    }

    /// Asks for one candidate for `request`.
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError>;

    /// [`SynthProvider::complete`], and what a response that held no reply showed (R-SYNTH-23, D-155). A provider that can
    /// tell overrides it and makes `complete` call it; the rest have nothing to add.
    async fn call(&self, request: &SynthRequest, limits: &SynthLimits) -> Called {
        Called {
            result: self.complete(request, limits).await,
            seen: None,
        }
    }
}

/// One provider call's outcome, with what the synth log keeps of a response that was read but held no reply.
#[derive(Debug)]
pub struct Called {
    /// The reply, or why there was none.
    pub result: Result<SynthReply, ProviderError>,
    /// Set only when `result` is an error.
    pub seen: Option<Seen>,
}

impl Called {
    /// A call that got no response to read: nothing was seen.
    pub(crate) fn unsent(error: ProviderError) -> Self {
        Self {
            result: Err(error),
            seen: None,
        }
    }

    /// The call whose transport gave `sent` (the response text) at `started`, read by `read`.
    pub(crate) fn from_sent(sent: Result<String, ProviderError>, started: Instant, read: fn(&str) -> Read) -> Self {
        let text = match sent {
            Ok(text) => text,
            Err(error) => return Self::unsent(error),
        };
        match read(&text) {
            Ok((reply_json, usage, meta)) => Self {
                result: Ok(SynthReply {
                    reply_json,
                    usage,
                    latency: started.elapsed(),
                    meta,
                }),
                seen: None,
            },
            Err((error, seen)) => Self {
                result: Err(error),
                seen,
            },
        }
    }
}

/// A response that gave no reply: why, and what it showed if it was read (D-155).
pub(crate) type Unread = (ProviderError, Option<Seen>);

/// The reply, usage and shape of a response that held one.
pub(crate) type Read = Result<(String, Usage, ReplyMeta), Unread>;

/// A failure after the response was read, which still shows how it looked and what it billed.
pub(crate) fn unread(why: ProviderError, meta: &ReplyMeta, usage: Usage) -> Unread {
    let seen = Seen {
        meta: meta.clone(),
        usage,
    };
    (why, Some(seen))
}

/// A response that was read but held no reply (a refusal, a cut-off, text only): its shape and the tokens it billed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Its stop reason and block kinds.
    pub meta: ReplyMeta,
    /// Its token counts.
    pub usage: Usage,
}

/// The default `max_output_tokens` of a provider that has no smaller one (`tooling/40` §5.1).
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;

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
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            synthesis_key,
            attempt: 0,
        }
    }
}

/// A provider's answer: one reply document, exactly as received (R-SYNTH-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthReply {
    /// The reply document: `{"body": <expression>}` or a question, not yet read or validated (D-103).
    pub reply_json: String,
    /// Tokens used.
    pub usage: Usage,
    /// How long the exchange took.
    pub latency: Duration,
    /// What the synth log keeps of how the reply came (R-SYNTH-23).
    pub meta: ReplyMeta,
}

/// What a provider says about a reply besides the reply and the token counts, as it said it. Only the synth log reads
/// it, and writes only its shape: untrusted values here are cut to closed sets before they leave `velme-synth`
/// (R-SYNTH-23, D-154). A provider that has none of it leaves the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyMeta {
    /// The provider's own stop reason: `anthropic`'s `stop_reason`, `ollama`'s `done_reason`.
    pub stop_reason: Option<String>,
    /// The `type` of each content block of the response in order, `""` for a block with none; `None` for a provider
    /// without blocks.
    pub blocks: Option<Vec<String>>,
}

/// Token counts of one exchange; zero where a provider doesn't report them (the cache counts are `None`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens sent.
    pub input_tokens: u64,
    /// Tokens received.
    pub output_tokens: u64,
    /// Prompt-cache tokens read (R-SYNTH-34): `None` when the provider doesn't report them, `Some(0)` when it reports 0
    /// (R-SYNTH-23).
    pub cache_read_tokens: Option<u64>,
    /// Prompt-cache tokens written (R-SYNTH-34), reported the same way.
    pub cache_write_tokens: Option<u64>,
}

/// Why a provider gave no reply (`compiler/22` R-SYNTH-07 maps each to a diagnostic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// No provider, model or key is configured, or the model isn't there (`VL0405`).
    NotConfigured,
    /// The Ollama server lacks the model named, as the user wrote it (`VL0405`, with the model in its words, so a missing
    /// `retry_model` is not reported as the primary). Replay fixtures and scripts spell it `not_configured`.
    ModelMissing(String),
    /// The provider rejected the API key that is set (`VL0405`, its own wording).
    KeyRejected,
    /// An external backend rejected the bearer token that is sent, with `401` or `403` (`VL0405`, its own wording).
    TokenRejected {
        /// Whether a token was sent; if not, the backend wants one.
        sent: bool,
    },
    /// The provider rejected the request with a `4xx` that no other variant names (`VL0405`, not retried, R-SYNTH-07).
    Rejected {
        /// The HTTP status; `None` from a replay fixture or script, which keep no status.
        status: Option<u16>,
        /// The API's own error message, with any key taken out, cleaned as one line and bounded; empty if it gave none.
        message: String,
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
    /// The reply was not a reply (a failed attempt, `VL0401`), for the reason given; a replayed or scripted one has none
    /// (R-SYNTH-07, D-153).
    Malformed(Option<MalformedReason>),
    /// An external backend failed (`VL0406`, R-SYNTH-28).
    BackendFailed {
        /// Why, in one cleaned line: Velme's wording, or the backend's own `{"error"}` text, collapsed and bounded.
        reason: String,
        /// The cleaned tail of the last 4 KiB of the backend's reply body, empty if there was none.
        body: String,
    },
    /// An `external` backend doesn't list the `reply_format` the project asks for in its `describe` reply (`VL0406`,
    /// R-SYNTH-26, D-156). Found before any request, so nothing was sent in the wrong format.
    FormatUnsupported {
        /// The backend's name from `describe`.
        backend: String,
        /// The formats it lists that Velme knows, by name.
        offered: Vec<&'static str>,
        /// The format `[synthesis] reply_format` asks for.
        asked: &'static str,
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

/// Why a reply was malformed: the closed list of R-SYNTH-07, each with an id and Velme's own words, never the provider's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedReason {
    /// The response passed the size limit.
    TooLarge,
    /// The response wasn't JSON, was nested too deeply or repeated a key.
    ResponseNotJson,
    /// The reply stopped at the output limit.
    CutOff,
    /// The response had no tool call, or more than one.
    NotOneTool,
    /// The tool called was neither of the two, or its input wasn't an object.
    UnknownTool,
    /// An `ollama` response without a message.
    NoMessage,
    /// The tool input or `ollama` reply wasn't JSON, was nested too deeply or repeated a key.
    ReplyNotJson,
    /// An `ollama` reply that isn't an object.
    ReplyNotObject,
}

impl MalformedReason {
    /// The reason's id, as the learner's cause and the synth log spell it (R-SYNTH-07, R-SYNTH-23).
    pub fn id(self) -> &'static str {
        match self {
            MalformedReason::TooLarge => "too-large",
            MalformedReason::ResponseNotJson => "response-not-json",
            MalformedReason::CutOff => "cut-off",
            MalformedReason::NotOneTool => "not-one-tool",
            MalformedReason::UnknownTool => "unknown-tool",
            MalformedReason::NoMessage => "no-message",
            MalformedReason::ReplyNotJson => "reply-not-json",
            MalformedReason::ReplyNotObject => "reply-not-object",
        }
    }

    /// Velme's words for it, a clause with its own subject: a fault of the HTTP response is "the response …", the rest
    /// "the reply …" (R-SYNTH-07).
    pub fn words(self) -> &'static str {
        match self {
            MalformedReason::TooLarge => "the response was too large",
            MalformedReason::ResponseNotJson => "the response wasn't JSON, or was nested too deeply or repeated a key",
            MalformedReason::CutOff => "the reply was cut off",
            MalformedReason::NotOneTool => "the reply didn't call exactly one tool",
            MalformedReason::UnknownTool => "the reply called a tool that isn't one of the two, or gave it no object",
            MalformedReason::NoMessage => "the response had no message",
            MalformedReason::ReplyNotJson => "the reply wasn't JSON, or was nested too deeply or repeated a key",
            MalformedReason::ReplyNotObject => "the reply wasn't an object",
        }
    }
}

impl ProviderError {
    /// The variant's name as replay fixtures and scripts spell it (R-SYNTH-43): `refused`, `pending`, …
    pub fn variant(&self) -> &'static str {
        match self {
            ProviderError::NotConfigured | ProviderError::ModelMissing(_) => "not_configured",
            ProviderError::KeyRejected => "key_rejected",
            ProviderError::TokenRejected { sent: true } => "token_rejected",
            ProviderError::TokenRejected { sent: false } => "token_required",
            ProviderError::Rejected { .. } => "rejected",
            ProviderError::Unavailable(_) => "unavailable",
            ProviderError::RateLimited { .. } => "rate_limited",
            ProviderError::Refused(_) => "refused",
            ProviderError::Timeout => "timeout",
            ProviderError::Malformed(_) => "malformed",
            ProviderError::BackendFailed { .. } | ProviderError::FormatUnsupported { .. } => "backend_failed",
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
            "rejected" => ProviderError::Rejected {
                status: None,
                message: String::new(),
            },
            "unavailable" => ProviderError::Unavailable(String::new()),
            "rate_limited" => ProviderError::RateLimited { retry_after: None },
            "refused" => ProviderError::Refused(String::new()),
            "timeout" => ProviderError::Timeout,
            "malformed" => ProviderError::Malformed(None),
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
