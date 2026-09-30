//! The `replay` provider (`compiler/22` R-SYNTH-05, R-SYNTH-43, D-94): answers from fixtures recorded in
//! `<replay_dir>`, so integration tests and golden builds need no live provider. A fixture holds, per exchange, the
//! hash of the request sent, the reply or error variant, and usage: never the prompt or the plan.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use velme_ir::{Fingerprint, from_json_str};

use crate::fsio::{read_file, refuse_links};
use crate::options::{ReplyFormat, RetryHistory, SynthOptions};
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::SynthRequest;

/// The file in `replay_dir` that holds the recorded build's identity.
pub const IDENTITY_FILE: &str = "replay.json";

/// The most a fixture file may hold: a goal's whole conversation, each reply within the IR limit, with room to spare.
/// A longer file is no fixture and is not read past this (R-ART-10's bound, applied to R-SYNTH-43).
const MAX_FIXTURE_BYTES: u64 = 16 * 1024 * 1024;

/// The hint that ends every replay failure (R-SYNTH-43).
const RE_RECORD: &str = "re-record the fixture with VELME_SYNTH_RECORD=1";

/// The `ProviderError` variants a fixture may hold: the ones that follow a reply reaching the provider's answer.
const RECORDED: [&str; 4] = ["refused", "malformed", "backend_failed", "pending"];

/// `replay.json`: the identity of the recorded build, which a replay reports so it computes the same keys and writes
/// the same manifests (R-SYNTH-43).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayIdentity {
    /// The provider id of the recorded build.
    pub provider: String,
    /// Its `model_version`.
    pub model_version: String,
    /// Its `input_version`.
    pub input_version: String,
    /// An external backend's name (R-ART-21), so a replayed build writes the same manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// `retry_history` when it wasn't the default `latest`: it changes the bytes of every retry request, which the
    /// fixtures are keyed on (R-SYNTH-43).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_history: Option<RetryHistory>,
    /// `reply_format` when it wasn't the default `ir-json`: it changes how the recorded replies are read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_format: Option<ReplyFormat>,
}

impl ReplayIdentity {
    /// Puts the settings that shaped the recorded requests and replies into `options`, over whatever they were: a
    /// replay must ask and read as the recording did, or its requests match no fixture (R-SYNTH-43).
    pub fn apply(&self, options: &mut SynthOptions) {
        options.retry_history = self.retry_history.unwrap_or_default();
        options.reply_format = self.reply_format.unwrap_or_default();
    }
}

/// The recorded build's identity, read from `<dir>/replay.json` like the artifact store reads its files: bounded, and
/// never through a link (`runtime/32` R-ART-10). `None` if there is no such file. Both the `replay` provider and the
/// CLI, which sets the settings a replay follows, read it here.
pub fn read_replay_identity(dir: &Path) -> Result<Option<ReplayIdentity>, ProviderError> {
    let path = dir.join(IDENTITY_FILE);
    let Some(text) = read_text(&path)? else {
        return Ok(None);
    };
    from_json_str(&text)
        .map(Some)
        .map_err(|_| ProviderError::Unavailable(format!("{IDENTITY_FILE} isn't a replay identity; {RE_RECORD}")))
}

/// Token counts as a fixture keeps them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureUsage {
    /// Tokens sent.
    pub input_tokens: u64,
    /// Tokens received.
    pub output_tokens: u64,
    /// Prompt-cache tokens read.
    pub cache_read_tokens: u64,
    /// Prompt-cache tokens written.
    pub cache_write_tokens: u64,
}

impl From<Usage> for FixtureUsage {
    fn from(usage: Usage) -> Self {
        FixtureUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

impl From<FixtureUsage> for Usage {
    fn from(usage: FixtureUsage) -> Self {
        Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

/// One exchange of a goal's fixture (R-SYNTH-43): a reply, or an error variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    /// The BLAKE3 of the canonical JSON of the request sent (`b3:<hex>`).
    pub request: Fingerprint,
    /// The reply document, as received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// The `ProviderError` variant: `refused`, `malformed`, `backend_failed` or `pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For `pending`, its cleaned text (R-SYNTH-41).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tokens used.
    pub usage: FixtureUsage,
}

/// The file of `key`'s fixture in `dir`: `b3-<hex>.json`, spelled as the artifact store spells its files (a `:` is not
/// a portable file name character).
pub fn fixture_path(dir: &Path, key: Fingerprint) -> PathBuf {
    dir.join(fixture_name(key))
}

/// The file name of `key`'s fixture.
pub(crate) fn fixture_name(key: Fingerprint) -> String {
    format!("b3-{}.json", key.hex())
}

/// A replay file that can't be read or written: `VL0901`.
pub(crate) fn file_error(path: &Path, error: &std::io::Error) -> ProviderError {
    ProviderError::File {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// The text of the replay file `path`, or `None` if there is no such file. Read like the artifact store reads: bounded,
/// and never through a link or from anything but a regular file (R-ART-10).
fn read_text(path: &Path) -> Result<Option<String>, ProviderError> {
    let Some(dir) = path.parent() else {
        return Ok(None);
    };
    let read = refuse_links(&[dir]).and_then(|()| read_file(path, MAX_FIXTURE_BYTES));
    match read {
        Ok(Some(bytes)) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| file_error(path, &std::io::Error::other(e))),
        Ok(None) => Err(file_error(
            path,
            &std::io::Error::other(format!(
                "it is longer than the {MAX_FIXTURE_BYTES} bytes a fixture may be"
            )),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(file_error(path, &e)),
    }
}

/// The replay backend over the fixtures in one directory.
#[derive(Debug, Clone)]
pub struct Replay {
    dir: PathBuf,
}

impl Replay {
    /// Replays the fixtures in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Replay { dir: dir.into() }
    }
}

#[async_trait]
impl SynthBackend for Replay {
    /// Reads `replay.json` (R-SYNTH-25). Contacts nothing else.
    async fn identify(&self) -> Result<Identity, ProviderError> {
        let identity = read_replay_identity(&self.dir)?.ok_or_else(|| {
            ProviderError::Unavailable(format!(
                "there is no {IDENTITY_FILE} in the replay directory; {RE_RECORD}"
            ))
        })?;
        Ok(Identity {
            provider: identity.provider,
            model: identity.model_version,
            input_version: identity.input_version,
            backend: identity.backend,
        })
    }

    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(ReplayProvider {
            dir: self.dir.clone(),
            identity: identity.clone(),
            used: Arc::default(),
        }))
    }
}

/// The provider a [`Replay`] opens: plays each goal's exchanges back in order.
#[derive(Debug)]
struct ReplayProvider {
    dir: PathBuf,
    identity: Identity,
    /// Exchanges already played, by goal.
    used: Arc<Mutex<BTreeMap<Fingerprint, usize>>>,
}

#[async_trait]
impl SynthProvider for ReplayProvider {
    fn id(&self) -> &str {
        &self.identity.provider
    }

    fn model(&self) -> &str {
        &self.identity.model
    }

    fn input_version(&self) -> &str {
        &self.identity.input_version
    }

    fn backend(&self) -> &str {
        self.identity.backend.as_deref().unwrap_or(&self.identity.provider)
    }

    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let key = limits.synthesis_key;
        let attempt = {
            let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
            let count = used.entry(key).or_insert(0);
            *count += 1;
            *count
        };
        let fail = |why: &str| {
            ProviderError::Unavailable(format!("replay fixture {key}, attempt {attempt}: {why}; {RE_RECORD}"))
        };
        let path = fixture_path(&self.dir, key);
        let text = read_text(&path)?.ok_or_else(|| fail("there is no fixture"))?;
        let exchanges: Vec<Exchange> =
            from_json_str(&text).map_err(|_| fail("the fixture isn't a list of exchanges"))?;
        let exchange = exchanges
            .get(attempt - 1)
            .ok_or_else(|| fail("the build asked for more attempts than were recorded"))?;
        let sent = request
            .hash()
            .map_err(|_| ProviderError::Internal("the request has no hash".to_owned()))?;
        if sent != exchange.request {
            return Err(fail("the request differs from the one recorded"));
        }
        if exchange.text.is_some() && exchange.error.as_deref() != Some("pending") {
            return Err(fail("an exchange has `text` without a `pending` error"));
        }
        match (&exchange.reply, &exchange.error) {
            (Some(reply), None) => Ok(SynthReply {
                reply_json: reply.clone(),
                usage: exchange.usage.into(),
                latency: Duration::ZERO,
            }),
            (None, Some(name)) if RECORDED.contains(&name.as_str()) => {
                match ProviderError::from_variant(name, exchange.text.as_deref()) {
                    Some(error) => Err(error),
                    None => Err(fail("an exchange names an unknown error")),
                }
            }
            (None, Some(_)) => Err(fail("an exchange names an error a recorder doesn't write")),
            _ => Err(fail("an exchange has both a reply and an error, or neither")),
        }
    }
}
