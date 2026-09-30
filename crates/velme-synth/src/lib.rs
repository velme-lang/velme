//! Velme `synth` crate: see `compiler/20` §2 for its responsibility. Spellbook (`compiler/22`): the provider-neutral
//! interface, the synthesis request, the prompt, and the providers that need no network (`scripted`, `replay`).
#![forbid(unsafe_code)]

mod prompt;
mod provider;
mod replay;
mod request;
mod schema;
mod scripted;

pub use prompt::{Prompt, Role, Turn, prompt_version, render};
pub use provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
pub use replay::{Exchange, FixtureUsage, IDENTITY_FILE, Replay, ReplayIdentity, fixture_path};
pub use request::{
    AttemptDiagnostic, AttemptFeedback, Budget, BuiltinSig, CheckItem, Example, ExternalMessage, LocalBinding, Param,
    REQUEST_VERSION, RecordType, Signature, SynthRequest, TaskKind, build_request, builtins, request_schema,
};
pub use schema::{reply_schema, schema_summary};
pub use scripted::{ScriptError, Scripted, Step};
