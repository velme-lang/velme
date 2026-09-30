//! Transport retries (`compiler/22` R-SYNTH-12, D-95): the waits every LLM provider and the external backend share.
//! A retry of this kind is inside one `complete()` call; it never consumes a synthesis retry and never counts as a call
//! (R-SYNTH-21).

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;

use crate::provider::ProviderError;

/// The waits before the first and second retry of a transport failure (R-SYNTH-12).
const WAITS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// The longest a `retry_after` is honoured (R-SYNTH-12).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Where the waits go: injected, so tests take no wall time (D-95).
#[async_trait]
pub trait Sleeper: Send + Sync {
    /// Waits `duration`.
    async fn sleep(&self, duration: Duration);
}

/// The host's clock: blocks the calling thread, which is all a build has to do while it waits.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdSleeper;

#[async_trait]
impl Sleeper for StdSleeper {
    async fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Runs `once`, and again up to twice after `RateLimited`, `Unavailable` or `Timeout`, waiting 1 s then 2 s, or a
/// `RateLimited`'s `retry_after` (at most 30 s) instead, with no jitter (R-SYNTH-12). Any other result is returned as it
/// comes; after the last retry the last error is.
pub async fn with_transport_retries<T, F, Fut>(sleeper: &dyn Sleeper, mut once: F) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut tries = 0;
    loop {
        let error = match once().await {
            Err(
                error @ (ProviderError::RateLimited { .. } | ProviderError::Unavailable(_) | ProviderError::Timeout),
            ) => error,
            other => return other,
        };
        let Some(wait) = WAITS.get(tries).copied() else {
            return Err(error);
        };
        tries += 1;
        let wait = match &error {
            ProviderError::RateLimited {
                retry_after: Some(after),
            } => (*after).min(MAX_RETRY_AFTER),
            _ => wait,
        };
        sleeper.sleep(wait).await;
    }
}
