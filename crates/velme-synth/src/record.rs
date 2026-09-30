//! The fixture recorder (`compiler/22` R-SYNTH-43, D-94): wraps any backend and writes every exchange that reached the
//! provider's reply as replay fixtures, so a `replay` build reproduces the recorded one byte for byte. A fixture holds
//! request hashes, replies or error variants, and usage: no prompt body, header or key (`tooling/41` R-SEC-07).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use velme_ir::{Fingerprint, to_canonical_string};

use crate::attempt::clean_line;
use crate::fsio::write_file;
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply};
use crate::replay::{Exchange, FixtureUsage, IDENTITY_FILE, ReplayIdentity, file_error, fixture_name};
use crate::request::SynthRequest;

/// A backend that records what `inner` answers into `dir`.
pub struct Recorder {
    inner: Box<dyn SynthBackend>,
    dir: PathBuf,
}

impl Recorder {
    /// Records the exchanges of `inner` in the replay directory `dir`.
    pub fn new(inner: Box<dyn SynthBackend>, dir: impl Into<PathBuf>) -> Self {
        Recorder { inner, dir: dir.into() }
    }
}

/// Writes the file `name` of the replay directory `dir` the way the artifact store writes: whole or not at all, and
/// never through a link (R-ART-09). A file that can't be written is `VL0901`, never a verdict on a candidate.
fn write(dir: &Path, name: &str, text: &str) -> Result<(), ProviderError> {
    write_file(dir, name, text.as_bytes()).map_err(|e| file_error(&dir.join(name), &e))
}

#[async_trait]
impl SynthBackend for Recorder {
    /// The inner identity, which `replay.json` then holds (R-SYNTH-43).
    async fn identify(&self) -> Result<Identity, ProviderError> {
        let identity = self.inner.identify().await?;
        let recorded = ReplayIdentity {
            provider: identity.provider.clone(),
            model_version: identity.model.clone(),
            input_version: identity.input_version.clone(),
            backend: identity.backend.clone(),
        };
        let text = to_canonical_string(&recorded)
            .map_err(|_| ProviderError::Internal("the replay identity could not be written".to_owned()))?;
        write(&self.dir, IDENTITY_FILE, &text)?;
        Ok(identity)
    }

    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(Recording {
            inner: self.inner.open(identity)?,
            dir: self.dir.clone(),
            seen: Mutex::default(),
        }))
    }
}

/// The provider a [`Recorder`] opens.
struct Recording {
    inner: Box<dyn SynthProvider>,
    dir: PathBuf,
    /// The exchanges of each goal so far, whose whole file is rewritten after each new one.
    seen: Mutex<BTreeMap<Fingerprint, Vec<Exchange>>>,
}

#[async_trait]
impl SynthProvider for Recording {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn input_version(&self) -> &str {
        self.inner.input_version()
    }

    fn backend(&self) -> &str {
        self.inner.backend()
    }

    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let result = self.inner.complete(request, limits).await;
        // Only what reached the provider's reply is an exchange: a transport failure is not (R-SYNTH-43).
        let (reply, error, text, usage) = match &result {
            Ok(reply) => (Some(reply.reply_json.clone()), None, None, reply.usage.into()),
            Err(
                e @ (ProviderError::Refused(_) | ProviderError::Malformed(_) | ProviderError::BackendFailed { .. }),
            ) => (None, Some(e.variant()), None, FixtureUsage::default()),
            Err(ProviderError::Pending(text)) => {
                (None, Some("pending"), Some(clean_line(text)), FixtureUsage::default())
            }
            Err(_) => return result,
        };
        let request = request
            .hash()
            .map_err(|_| ProviderError::Internal("the request has no hash".to_owned()))?;
        let file = {
            let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
            let all = seen.entry(limits.synthesis_key).or_default();
            all.push(Exchange {
                request,
                reply,
                error: error.map(str::to_owned),
                text,
                usage,
            });
            to_canonical_string(&*all)
                .map_err(|_| ProviderError::Internal("a replay fixture could not be written".to_owned()))?
        };
        write(&self.dir, &fixture_name(limits.synthesis_key), &file)?;
        result
    }
}
