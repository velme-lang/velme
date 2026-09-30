//! What the HTTP providers (`anthropic`, `ollama`, `external`) share: the body cap, the depth allowance for a response envelope, the
//! agent, and how a failed exchange maps to a [`ProviderError`] (`compiler/22` R-SYNTH-12, D-98). No error text here holds
//! an address or anything the server sent (R-SYNTH-22).

use std::time::Duration;

use crate::provider::ProviderError;

/// The most a response body may hold: far above `max_output_tokens` of text, far below a memory problem.
pub(crate) const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The host part of `url`: `127.0.0.1` for `http://127.0.0.1:11434/x`; empty for text that is not a URL.
#[cfg(any(feature = "test-endpoint", feature = "provider-ollama", test))]
pub(crate) fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

/// Whether `url` names this machine.
#[cfg(any(feature = "provider-ollama", test))]
pub(crate) fn is_loopback(url: &str) -> bool {
    matches!(host_of(url), "127.0.0.1" | "localhost" | "::1")
}

/// The settings of an agent that gives up after `timeout`, follows no redirect and reads a non-2xx status as a response,
/// not an error. A server on this machine is never reached through a proxy from the environment.
pub(crate) fn agent_config(timeout: Duration, direct: bool) -> ureq::config::Config {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0);
    if direct {
        config = config.proxy(None);
    }
    config.build()
}

/// [`agent_config`] as an agent.
#[cfg(any(feature = "provider-anthropic", feature = "provider-ollama"))]
pub(crate) fn agent(timeout: Duration, direct: bool) -> ureq::Agent {
    agent_config(timeout, direct).into()
}

/// The body of a success response, at most [`MAX_BODY_BYTES`].
pub(crate) fn read_body(response: &mut ureq::http::Response<ureq::Body>) -> Result<String, ProviderError> {
    response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|error| match error {
            ureq::Error::BodyExceedsLimit(_) => ProviderError::Malformed("the response was too large".to_owned()),
            other => transport_error(other),
        })
}

/// What a failed HTTP exchange means. The text is fixed: a transport error's own wording may hold the address.
pub(crate) fn transport_error(error: ureq::Error) -> ProviderError {
    match error {
        ureq::Error::Timeout(_) => ProviderError::Timeout,
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => ProviderError::Timeout,
        // A header that can't be built, such as a key with a control character, is never a network failure.
        ureq::Error::Http(_) | ureq::Error::BadUri(_) => ProviderError::NotConfigured,
        _ => ProviderError::Unavailable("Velme couldn't connect to the provider".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{host_of, is_loopback};

    #[test]
    fn the_host_of_a_url_is_found_and_only_this_machine_is_loopback() {
        assert_eq!(host_of("http://127.0.0.1:11434/api"), "127.0.0.1");
        assert_eq!(host_of("http://user@example.com:80"), "example.com");
        assert_eq!(host_of("http://[::1]:8080/x"), "::1");
        assert_eq!(host_of("not a url"), "");
        assert!(is_loopback("http://localhost:1"));
        assert!(is_loopback("http://[::1]:1"));
        assert!(!is_loopback("http://127.0.0.1.evil.example/"));
        assert!(!is_loopback("http://evil.example/127.0.0.1"));
    }
}
