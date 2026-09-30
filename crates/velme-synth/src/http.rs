//! What the HTTP providers (`anthropic`, `ollama`, `external`) share: the body cap, the depth allowance for a response envelope, the
//! agent, and how a failed exchange maps to a [`ProviderError`] (`compiler/22` R-SYNTH-12, D-98). No error text here holds
//! an address or anything the server sent (R-SYNTH-22).

use std::time::Duration;

use crate::provider::ProviderError;

/// The most a response body may hold: far above `max_output_tokens` of text, far below a memory problem.
pub(crate) const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The most that resolving a name, and then connecting, may each take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The host part of `url`: `127.0.0.1` for `http://127.0.0.1:11434/x`; empty for text that is not a URL.
#[cfg(any(feature = "test-endpoint", test))]
pub(crate) fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

/// The settings of an agent that gives up after `timeout`, follows no redirect and reads a non-2xx status as a response,
/// not an error. A server on this machine is never reached through a proxy from the environment.
pub(crate) fn agent_config(timeout: Duration, direct: bool) -> ureq::config::Config {
    agent_config_trusting(timeout, direct, &[])
}

/// The certificates a connection trusts when `external_ca_file` gave `extra`: the bundled roots and then those (D-105).
pub(crate) fn root_set(extra: &[ureq::tls::Certificate<'static>]) -> Vec<ureq::tls::Certificate<'static>> {
    let bundled = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der.as_ref()));
    bundled.chain(extra.iter().cloned()).collect()
}

/// [`agent_config`] that trusts `roots` in addition to the bundled ones, when there are any (`external_ca_file`, D-105).
pub(crate) fn agent_config_trusting(
    timeout: Duration,
    direct: bool,
    roots: &[ureq::tls::Certificate<'static>],
) -> ureq::config::Config {
    let connect = CONNECT_TIMEOUT.min(timeout / 2);
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // Reaching the server is its own, shorter limit, so a connect that never completes is `Unavailable` (retried) and
        // only a timeout after the request was sent is a `Timeout` (R-SYNTH-12, D-110).
        .timeout_resolve(Some(connect))
        .timeout_connect(Some(connect))
        .http_status_as_error(false)
        .max_redirects(0);
    if direct {
        config = config.proxy(None);
    }
    if !roots.is_empty() {
        let all = root_set(roots);
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&all));
        config = config.tls_config(tls.build());
    }
    config.build()
}

/// [`agent_config`] as an agent.
#[cfg(feature = "provider-anthropic")]
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

/// A timeout while resolving or connecting is a failure to reach the server; any later one is a [`ProviderError::Timeout`].
fn connect_phase_timeout(kind: &ureq::Timeout) -> ProviderError {
    match kind {
        ureq::Timeout::Resolve | ureq::Timeout::Connect => {
            ProviderError::Unavailable("Velme couldn't connect to the provider".to_owned())
        }
        _ => ProviderError::Timeout,
    }
}

/// What a failed HTTP exchange means. The text is fixed: a transport error's own wording may hold the address.
pub(crate) fn transport_error(error: ureq::Error) -> ProviderError {
    match error {
        ureq::Error::Timeout(kind) => connect_phase_timeout(&kind),
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => ProviderError::Timeout,
        // A header that can't be built, such as a key with a control character, is never a network failure.
        ureq::Error::Http(_) | ureq::Error::BadUri(_) => ProviderError::NotConfigured,
        _ => ProviderError::Unavailable("Velme couldn't connect to the provider".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{connect_phase_timeout, host_of, root_set};
    use crate::provider::ProviderError;

    /// Only a timeout after the request was sent is a `Timeout` (R-SYNTH-12, D-110).
    #[test]
    fn a_timeout_before_the_request_was_sent_is_unavailable() {
        for kind in [ureq::Timeout::Resolve, ureq::Timeout::Connect] {
            assert!(matches!(connect_phase_timeout(&kind), ProviderError::Unavailable(_)));
        }
        for kind in [
            ureq::Timeout::Global,
            ureq::Timeout::RecvResponse,
            ureq::Timeout::RecvBody,
        ] {
            assert!(matches!(connect_phase_timeout(&kind), ProviderError::Timeout));
        }
    }

    /// `external_ca_file` is added to the bundled roots, not put in their place (D-105).
    #[test]
    fn the_ca_file_is_added_to_the_bundled_roots() {
        let bundled = root_set(&[]).len();
        assert!(bundled > 100, "the bundled roots are present");
        let extra = ureq::tls::Certificate::from_der(&[0x30, 0x03, 0x02, 0x01, 0x00]).to_owned();
        assert_eq!(root_set(&[extra]).len(), bundled + 1);
    }

    #[test]
    fn the_host_of_a_url_is_found() {
        assert_eq!(host_of("http://127.0.0.1:11434/api"), "127.0.0.1");
        assert_eq!(host_of("http://user@example.com:80"), "example.com");
        assert_eq!(host_of("http://[::1]:8080/x"), "::1");
        assert_eq!(host_of("not a url"), "");
    }
}
