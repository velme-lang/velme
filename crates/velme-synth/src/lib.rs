//! Velme `synth` crate: see `compiler/20` §2 for its responsibility. Spellbook (`compiler/22`): the provider-neutral
//! interface, the synthesis request, the prompt, and the providers (`anthropic`, `ollama`, `external`, and `scripted` and
//! `replay`, which need no network) and the fixture recorder.
#![forbid(unsafe_code)]

#[cfg(feature = "provider-anthropic")]
mod anthropic;
mod attempt;
mod compact;
#[cfg(feature = "provider-external")]
mod external;
pub mod fsio;
mod generate;
#[cfg(any(
    feature = "provider-anthropic",
    feature = "provider-ollama",
    feature = "provider-external"
))]
mod http;
#[cfg(feature = "provider-ollama")]
mod ollama;
mod options;
mod prompt;
mod provider;
mod record;
mod replay;
mod request;
mod schema;
mod scripted;
mod synthesize;
mod transport;
mod verify;

#[cfg(feature = "provider-anthropic")]
pub use anthropic::{Anthropic, AnthropicConfig, ApiKey, KeyError};
pub use attempt::{Cause, Rejection};
pub use compact::{AliasTable, UnknownAlias, compress, expand, table as alias_table};
#[cfg(feature = "provider-external")]
pub use external::{External, ExternalConfig, ExternalToken, ExternalUrl, TOKEN_VARIABLE, TokenMalformed, UrlError};
pub use generate::{TestInput, TestInputs, test_inputs};
#[cfg(feature = "provider-ollama")]
pub use ollama::{DEFAULT_URL as OLLAMA_URL, Ollama, OllamaConfig, normalize_model};
pub use options::{PromptOptions, ReplyFormat, RetryHistory, SchemaInPrompt, SynthOptions};
pub use prompt::{Prompt, Role, Turn, prompt_version, render, render_with};
pub use provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
pub use record::Recorder;
pub use replay::{Exchange, FixtureUsage, IDENTITY_FILE, Replay, ReplayIdentity, fixture_path, read_replay_identity};
pub use request::{
    AttemptDiagnostic, AttemptFeedback, Budget, BuiltinSig, CheckItem, Example, LocalBinding, Param, REQUEST_VERSION,
    RecordType, Signature, SynthRequest, TaskKind, build_request, builtins, request_schema,
};
pub use schema::{reply_schema, schema_summary};
pub use scripted::{ScriptError, Scripted, Step};
pub use synthesize::{
    Built, Failure, Outcome, Session, Task, provider_diagnostic, reaches_no_further, stopped_diagnostic, synthesize,
    unavailable,
};
pub use transport::{Sleeper, StdSleeper, with_transport_retries};
pub use verify::{ChildRunner, Verdict, Verified, verify};
