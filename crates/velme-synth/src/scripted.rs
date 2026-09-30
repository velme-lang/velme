//! The `scripted` provider (`compiler/22` R-SYNTH-05): a queue of replies and errors, for tests of the retry loop and
//! the verification pipeline. Library tests fill the queue in memory; a `velme-cli` built with the `test-provider`
//! feature reads it from the script file `VELME_SYNTH_SCRIPT` names (D-94), which [`Scripted::from_script`] parses.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use velme_ir::{from_json_str, to_canonical_string};

use crate::prompt::prompt_version;
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::SynthRequest;

/// One scripted answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A reply document, returned exactly as written.
    Reply(String),
    /// An error in place of a reply.
    Error(ProviderError),
}

/// What a script file holds that is not a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the synth script {}", self.0)
    }
}

impl std::error::Error for ScriptError {}

#[derive(Debug, Default)]
struct State {
    steps: VecDeque<Step>,
    requests: Vec<SynthRequest>,
    limits: Vec<SynthLimits>,
}

/// A scripted backend and provider: each `complete()` answers with the next step and remembers what it was asked. A
/// clone shares the queue, so the provider a build opens is the one a test inspects.
#[derive(Debug, Clone)]
pub struct Scripted {
    state: Arc<Mutex<State>>,
    input_version: String,
}

impl Scripted {
    /// A provider that answers with `steps` in order.
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Scripted {
            state: Arc::new(Mutex::new(State {
                steps: steps.into_iter().collect(),
                ..State::default()
            })),
            input_version: prompt_version(),
        }
    }

    /// A provider that answers with replies, each returned as written.
    pub fn replies<S: Into<String>>(replies: impl IntoIterator<Item = S>) -> Self {
        Self::new(replies.into_iter().map(|r| Step::Reply(r.into())))
    }

    /// The queue a script file describes (D-94): a JSON array in which each entry is a reply, as a JSON string (returned
    /// verbatim, so it can be invalid JSON) or any other JSON value (returned as its canonical JSON), or
    /// `{"error": "<variant>"}`, with `"text"` for `pending`.
    pub fn from_script(text: &str) -> Result<Self, ScriptError> {
        let entries: Vec<Value> = from_json_str(text).map_err(|e| ScriptError(format!("isn't a JSON array: {e}")))?;
        let steps = entries
            .into_iter()
            .enumerate()
            .map(|(n, entry)| step(entry).map_err(|e| ScriptError(format!("entry {}: {e}", n + 1))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(steps))
    }

    /// How many calls have been made.
    pub fn calls(&self) -> usize {
        self.lock().requests.len()
    }

    /// The request of each call, in order.
    pub fn requests(&self) -> Vec<SynthRequest> {
        self.lock().requests.clone()
    }

    /// The limits of each call, in order.
    pub fn limits(&self) -> Vec<SynthLimits> {
        self.lock().limits.clone()
    }

    /// The steps not yet used.
    pub fn remaining(&self) -> usize {
        self.lock().steps.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn step(entry: Value) -> Result<Step, String> {
    match entry {
        Value::String(reply) => Ok(Step::Reply(reply)),
        Value::Object(map) if map.contains_key("error") => {
            if let Some(key) = map.keys().find(|k| *k != "error" && *k != "text") {
                return Err(format!("an error entry has no `{key}` key"));
            }
            let name = map
                .get("error")
                .and_then(Value::as_str)
                .ok_or("`error` isn't a variant name")?;
            if map.contains_key("text") && name != "pending" {
                return Err("`text` belongs to a `pending` error only".to_owned());
            }
            let text = match map.get("text") {
                None => None,
                Some(Value::String(text)) => Some(text.as_str()),
                Some(_) => return Err("`text` isn't a string".to_owned()),
            };
            let error =
                ProviderError::from_variant(name, text).ok_or_else(|| format!("`{name}` isn't an error variant"))?;
            Ok(Step::Error(error))
        }
        other => to_canonical_string(&other)
            .map(Step::Reply)
            .map_err(|e| format!("isn't canonical JSON: {e}")),
    }
}

#[async_trait]
impl SynthBackend for Scripted {
    async fn identify(&self) -> Result<Identity, ProviderError> {
        Ok(Identity {
            provider: "scripted".to_owned(),
            model: "scripted".to_owned(),
            input_version: self.input_version.clone(),
        })
    }

    fn open(&self, _identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl SynthProvider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    fn model(&self) -> &str {
        "scripted"
    }

    fn input_version(&self) -> &str {
        &self.input_version
    }

    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let mut state = self.lock();
        state.requests.push(request.clone());
        state.limits.push(limits.clone());
        match state.steps.pop_front() {
            Some(Step::Reply(reply_json)) => Ok(SynthReply {
                reply_json,
                usage: Usage::default(),
                latency: Duration::ZERO,
            }),
            Some(Step::Error(error)) => Err(error),
            None => Err(ProviderError::Unavailable(
                "the scripted provider has no reply left".to_owned(),
            )),
        }
    }
}
