//! Velme `synth` crate: see `compiler/20` §2 for its responsibility. Spellbook (`compiler/22`): the provider-neutral
//! interface, the synthesis request, the prompt, and the providers that need no network (`scripted`, `replay`).
#![forbid(unsafe_code)]

mod attempt;
mod compact;
mod generate;
mod options;
mod prompt;
mod provider;
mod replay;
mod request;
mod schema;
mod scripted;
mod synthesize;
mod transport;
mod verify;

pub use attempt::{Cause, Rejection};
pub use compact::{AliasTable, UnknownAlias, compress, expand, table as alias_table};
pub use generate::{TestInput, TestInputs, test_inputs};
pub use options::{PromptOptions, ReplyFormat, RetryHistory, SchemaInPrompt, SynthOptions};
pub use prompt::{Prompt, Role, Turn, prompt_version, render, render_with};
pub use provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
pub use replay::{Exchange, FixtureUsage, IDENTITY_FILE, Replay, ReplayIdentity, fixture_path};
pub use request::{
    AttemptDiagnostic, AttemptFeedback, Budget, BuiltinSig, CheckItem, Example, ExternalMessage, LocalBinding, Param,
    REQUEST_VERSION, RecordType, Signature, SynthRequest, TaskKind, build_request, builtins, request_schema,
};
pub use schema::{reply_schema, schema_summary};
pub use scripted::{ScriptError, Scripted, Step};
pub use synthesize::{
    Built, Failure, Outcome, Session, Task, provider_diagnostic, reaches_no_further, synthesize, unavailable,
};
pub use transport::{Sleeper, StdSleeper, with_transport_retries};
pub use verify::{ChildRunner, Verdict, Verified, verify};
