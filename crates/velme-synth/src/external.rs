//! The `external` provider (`compiler/22` §3.2, R-SYNTH-26..29, R-SYNTH-41, D-42, D-45, D-98, D-101): a standalone
//! service the user starts, spoken to over HTTP and JSON at a URL that comes from the user, never from a project file
//! (`tooling/40` R-CLI-13). Velme starts no process. Whatever the service says is untrusted: the reply goes through the
//! same validation and verification as an LLM's (R-SYNTH-27), and the bearer token, if any, is sent to that URL alone
//! (`tooling/41` R-SEC-13).

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Map, Value};
use ureq::config::Config as UreqConfig;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use velme_ir::{MAX_JSON_DEPTH, from_json_str_within, to_canonical_string};

use crate::attempt::{clean_line, clean_name, clean_tail};
use crate::http::{agent_config_trusting, transport_error};
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::{REQUEST_VERSION, SynthRequest};
use crate::transport::{ENVELOPE_DEPTH, Sleeper, StdSleeper, with_transport_retries};

/// The most a service may send for one reply (R-SYNTH-28).
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// The provider id of the backend (R-SYNTH-26).
const PROVIDER: &str = "external";

/// How much of a service's reply body the notes keep (R-SYNTH-28).
const BODY_TAIL_BYTES: usize = 4096;

/// The longest a backend's name or version may be after cleaning, in Unicode scalar values (R-SYNTH-26).
const MAX_NAME_CHARS: usize = 128;

/// The shortest token Velme accepts: a shorter one could not be told from the text of a reply, so redacting it would
/// mean nothing (D-102).
const MIN_TOKEN_CHARS: usize = 16;

/// The environment variable that holds the bearer token (`tooling/40` §5.2).
pub const TOKEN_VARIABLE: &str = "VELME_EXTERNAL_TOKEN";

/// Why a URL can't be used for the `external` backend (`compiler/22` R-SYNTH-29, `VL0902`). The text names the problem, never
/// the URL, which may have been mistyped with a secret in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Not a URL of the form `scheme://host[:port][/path]`.
    Unparsable,
    /// A scheme other than `http` or `https`.
    Scheme,
    /// The URL carries a user name or password.
    Userinfo,
    /// Plain `http` to a host that isn't this machine.
    Insecure,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UrlError::Unparsable => "it isn't a URL of the form scheme://host[:port][/path]",
            UrlError::Scheme => "its scheme isn't http or https",
            UrlError::Userinfo => "it holds a user name or password",
            UrlError::Insecure => "plain http is allowed only for localhost, 127.0.0.0/8 and [::1]",
        })
    }
}

/// The base URL of an `external` service, checked (R-SYNTH-29): `https`, or `http` for this machine only, with no user
/// information, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalUrl {
    /// The URL without a trailing slash.
    text: String,
    /// The host as written, brackets included for an IPv6 address.
    host: String,
    loopback: bool,
}

impl ExternalUrl {
    /// `text` checked, or why it can't be used. Nothing is contacted.
    pub fn parse(text: &str) -> Result<Self, UrlError> {
        let text = text.trim();
        if text.chars().any(|c| c.is_control() || c.is_whitespace()) || !text.is_ascii() {
            return Err(UrlError::Unparsable);
        }
        let (scheme, rest) = text.split_once("://").ok_or(UrlError::Unparsable)?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            s if !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) =>
            {
                return Err(UrlError::Scheme);
            }
            _ => return Err(UrlError::Unparsable),
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(end);
        if authority.contains('@') {
            return Err(UrlError::Userinfo);
        }
        if !path_is_valid(path) {
            return Err(UrlError::Unparsable);
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (inside, after) = v6.split_once(']').ok_or(UrlError::Unparsable)?;
                if inside.parse::<Ipv6Addr>().is_err() || !(after.is_empty() || after.starts_with(':')) {
                    return Err(UrlError::Unparsable);
                }
                (format!("[{inside}]"), after.strip_prefix(':'))
            }
            None => match authority.split_once(':') {
                Some((host, port)) => (host.to_owned(), Some(port)),
                None => (authority.to_owned(), None),
            },
        };
        if let Some(port) = port
            && (port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err())
        {
            return Err(UrlError::Unparsable);
        }
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let name_ok = !bare.is_empty()
            && (host.starts_with('[')
                || bare
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')));
        if !name_ok || bare.starts_with(['.', '-']) || bare.ends_with('-') || bare.contains("..") {
            return Err(UrlError::Unparsable);
        }
        let loopback = bare.eq_ignore_ascii_case("localhost")
            || bare.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
            || bare.parse::<Ipv6Addr>().is_ok_and(|ip| ip == Ipv6Addr::LOCALHOST);
        if !secure && !loopback {
            return Err(UrlError::Insecure);
        }
        Ok(ExternalUrl {
            text: format!("{scheme}://{authority}{}", path.trim_end_matches('/')),
            host,
            loopback,
        })
    }

    /// The host, for the R-SEC-12 notice: a name, or an address in brackets. Its characters are checked, so it needs no
    /// escaping.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The URL of the endpoint `path`, which starts with `/`.
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.text)
    }
}

/// A bearer token (`tooling/41` R-SEC-05, R-SEC-06). Its `Debug` and `Display` print `***`; the text leaves it only into the
/// `Authorization` header of a request to the configured URL.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalToken(String);

/// Why the environment holds no token to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenMalformed;

impl ExternalToken {
    /// The token in [`TOKEN_VARIABLE`], trimmed: `None` when it is unset or empty. A value that can't be a token is an
    /// error, never left out, so a typo never sends a request without the token the user meant to send.
    pub fn lookup() -> Result<Option<Self>, TokenMalformed> {
        match std::env::var(TOKEN_VARIABLE) {
            Ok(value) => Self::parse(&value),
            Err(std::env::VarError::NotUnicode(_)) => Err(TokenMalformed),
            Err(std::env::VarError::NotPresent) => Ok(None),
        }
    }

    /// `value` as a token: empty once trimmed is none; anything but visible ASCII, or fewer than 16 characters, is
    /// [`TokenMalformed`].
    pub fn parse(value: &str) -> Result<Option<Self>, TokenMalformed> {
        let value = value.trim();
        if value.is_empty() {
            Ok(None)
        } else if value.len() >= MIN_TOKEN_CHARS && value.bytes().all(|b| b.is_ascii_graphic()) {
            Ok(Some(ExternalToken(value.to_owned())))
        } else {
            Err(TokenMalformed)
        }
    }
}

impl fmt::Debug for ExternalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExternalToken(***)")
    }
}

impl fmt::Display for ExternalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// The settings of the `external` provider (`tooling/40` §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConfig {
    /// The service's base URL.
    pub url: ExternalUrl,
    /// The bearer token, if the service wants one.
    pub token: Option<ExternalToken>,
    /// The most time any one request may take, `describe` included (`external_timeout_secs`, R-SYNTH-28).
    pub timeout: Duration,
    /// The PEM certificates of `external_ca_file` (D-105), trusted for this backend's connections only.
    pub ca_pem: Option<Vec<u8>>,
}

impl ExternalConfig {
    /// The default `external_timeout_secs`.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// `url`, with no token and the default timeout.
    pub fn new(url: ExternalUrl) -> Self {
        ExternalConfig {
            url,
            token: None,
            timeout: Self::DEFAULT_TIMEOUT,
            ca_pem: None,
        }
    }
}

/// The certificates of the PEM text `pem` (`external_ca_file`); empty when it holds none.
fn certificates(pem: &[u8]) -> Vec<ureq::tls::Certificate<'static>> {
    ureq::tls::parse_pem(pem)
        .filter_map(|item| match item {
            Ok(ureq::tls::PemItem::Certificate(certificate)) => Some(certificate),
            _ => None,
        })
        .collect()
}

/// Whether `pem` holds at least one certificate, as an `external_ca_file` must (`tooling/40` §5.1).
pub fn has_certificate(pem: &[u8]) -> bool {
    !certificates(pem).is_empty()
}

/// Whether `path` is made only of RFC 3986 `pchar`s and `/`: unreserved characters, percent-escapes, sub-delimiters, `:` and
/// `@`. A character such as `"`, `<`, `{` or `\` is refused here, before any contact, and not as a bad URI at send time.
fn path_is_valid(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        let plain = b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b);
        if b == b'%' {
            let escape = bytes.get(at + 1..at + 3);
            if !escape.is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)) {
                return false;
            }
            at += 3;
        } else if plain {
            at += 1;
        } else {
            return false;
        }
    }
    true
}

/// The addresses `localhost` stands for, without asking DNS: 127.0.0.1 and `::1` (R-SYNTH-29). A resolver, an `/etc/hosts`
/// entry or a search domain can't send a request meant for this machine somewhere else.
fn localhost_addrs(port: u16) -> [SocketAddr; 2] {
    [
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
    ]
}

/// Resolves `localhost` to [`localhost_addrs`] and every other host as usual.
#[derive(Debug)]
struct LocalResolver;

impl Resolver for LocalResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &UreqConfig,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if uri.host().is_some_and(|host| host.eq_ignore_ascii_case("localhost")) {
            let port = uri
                .port_u16()
                .unwrap_or(if uri.scheme_str() == Some("https") { 443 } else { 80 });
            let mut found = self.empty();
            for addr in localhost_addrs(port) {
                found.push(addr);
            }
            return Ok(found);
        }
        DefaultResolver::default().resolve(uri, config, timeout)
    }
}

/// What is asked of the service.
enum Call<'a> {
    /// `GET /v1/describe` (R-SYNTH-26).
    Describe,
    /// `POST /v1/synthesize` with the canonical request as the body.
    Synthesize(&'a str),
}

/// The `external` backend. Its identity step asks `describe` (R-SYNTH-25, R-SYNTH-26).
#[derive(Clone)]
pub struct External {
    config: ExternalConfig,
    sleeper: Arc<dyn Sleeper>,
}

impl fmt::Debug for External {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("External")
            .field("host", &self.config.url.host)
            .finish_non_exhaustive()
    }
}

/// A backend failure with its reason and the cleaned tail of the reply body (R-SYNTH-28).
fn failed(reason: &str, body: &str) -> ProviderError {
    ProviderError::BackendFailed {
        reason: reason.to_owned(),
        body: body.to_owned(),
    }
}

impl External {
    /// The backend for `config`.
    pub fn new(config: ExternalConfig) -> Self {
        External {
            config,
            sleeper: Arc::new(StdSleeper),
        }
    }

    /// Waits between transport retries on `sleeper` instead of the host's clock (D-95).
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// `text` without the token, if the service echoed it, in either its plain or its JSON-escaped spelling. Every message
    /// text that comes from the service passes through here before it is kept or shown (`tooling/41` R-SEC-13, D-102).
    fn redact(&self, text: &str) -> String {
        let Some(token) = &self.config.token else {
            return text.to_owned();
        };
        let mut out = text.replace(token.0.as_str(), "***");
        if let Ok(escaped) = serde_json::to_string(&token.0)
            && let Some(inner) = escaped.get(1..escaped.len().saturating_sub(1))
            && inner != token.0
        {
            out = out.replace(inner, "***");
        }
        out
    }

    /// Whether `text`, JSON, holds the token in either spelling.
    fn holds_token(&self, text: &str) -> bool {
        self.redact(text) != text
    }

    /// The tail of `bytes` as the notes carry it: the token is taken out of the whole text first, so one cut by the start
    /// of the tail can't leave part of it, then the last 4 KiB is cleaned.
    fn tail(&self, bytes: &[u8]) -> String {
        let whole = self.redact(&String::from_utf8_lossy(bytes));
        clean_tail(whole.as_bytes(), BODY_TAIL_BYTES)
    }

    /// One HTTP exchange: the whole body of a success. A `5xx` leaves the tail of its body in `server` for the failure the
    /// retries end with.
    fn once(&self, call: &Call<'_>, server: &Mutex<Option<(u16, String)>>) -> Result<Vec<u8>, ProviderError> {
        if let Ok(mut held) = server.lock() {
            *held = None;
        }
        let roots = self.config.ca_pem.as_deref().map(certificates).unwrap_or_default();
        let agent = ureq::Agent::with_parts(
            agent_config_trusting(self.config.timeout, self.config.url.loopback, &roots),
            DefaultConnector::default(),
            LocalResolver,
        );
        let auth = self.config.token.as_ref().map(|token| format!("Bearer {}", token.0));
        let mut response = match call {
            Call::Describe => {
                let mut request = agent
                    .get(&self.config.url.endpoint("/v1/describe"))
                    .header("accept", "application/json");
                if let Some(auth) = &auth {
                    request = request.header("authorization", auth.as_str());
                }
                request.call()
            }
            Call::Synthesize(body) => {
                let mut request = agent
                    .post(&self.config.url.endpoint("/v1/synthesize"))
                    .header("content-type", "application/json")
                    .header("accept", "application/json");
                if let Some(auth) = &auth {
                    request = request.header("authorization", auth.as_str());
                }
                request.send(*body)
            }
        }
        .map_err(transport_error)?;
        let status = response.status().as_u16();
        let (bytes, over) = read_capped(&mut response).map_err(|error| {
            // ureq wraps its own timeout in an `io::Error` of another kind.
            let wrapped = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ureq::Error>())
                .is_some_and(|inner| matches!(inner, ureq::Error::Timeout(_)));
            if wrapped || error.kind() == std::io::ErrorKind::TimedOut {
                ProviderError::Timeout
            } else {
                ProviderError::Unavailable("Velme couldn't read the reply".to_owned())
            }
        })?;
        match status {
            200..=299 if over => Err(failed(
                &format!("it wrote more than {} MiB", MAX_BODY_BYTES / (1024 * 1024)),
                &self.tail(&bytes),
            )),
            200..=299 => Ok(bytes),
            401 | 403 => Err(ProviderError::TokenRejected {
                sent: self.config.token.is_some(),
            }),
            429 => Err(ProviderError::RateLimited { retry_after: None }),
            408 | 500..=599 => {
                if let Ok(mut held) = server.lock() {
                    *held = Some((status, self.tail(&bytes)));
                }
                Err(ProviderError::Unavailable(format!(
                    "the backend answered with status {status}"
                )))
            }
            300..=399 => Err(failed(
                &format!("it answered with a redirect (status {status}), which Velme doesn't follow"),
                &self.tail(&bytes),
            )),
            _ => Err(failed(&format!("it answered with status {status}"), &self.tail(&bytes))),
        }
    }

    /// Sends `call`, retrying a transport failure as every provider does (R-SYNTH-12), and reads its one JSON object
    /// with the cleaned tail of the body, which every failure that follows carries (R-SYNTH-28).
    async fn exchange(&self, call: &Call<'_>) -> Result<(Map<String, Value>, String), ProviderError> {
        let server = Mutex::new(None);
        let sent = with_transport_retries(self.sleeper.as_ref(), || std::future::ready(self.once(call, &server))).await;
        let bytes = match sent {
            Ok(bytes) => bytes,
            // A server error that outlived the retries is the backend failing, not the network (R-SYNTH-28).
            Err(ProviderError::Unavailable(why)) => {
                let held = server.lock().ok().and_then(|mut held| held.take());
                return Err(match held {
                    Some((status, tail)) => failed(&format!("it answered with status {status}"), &tail),
                    None => ProviderError::Unavailable(why),
                });
            }
            Err(error) => return Err(error),
        };
        let tail = self.tail(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| failed("its reply wasn't text", &tail))?;
        match from_json_str_within::<Value>(&text, MAX_JSON_DEPTH + ENVELOPE_DEPTH) {
            Ok(Value::Object(doc)) => Ok((doc, tail)),
            _ => Err(failed(
                "its reply wasn't one JSON object, or nested too deeply or repeated a key",
                &tail,
            )),
        }
    }
}

/// The body of `response` up to [`MAX_BODY_BYTES`], and whether there was more.
fn read_capped(response: &mut ureq::http::Response<ureq::Body>) -> Result<(Vec<u8>, bool), std::io::Error> {
    use std::io::Read;
    let mut reader = response.body_mut().as_reader();
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            return Ok((kept, false));
        }
        kept.extend_from_slice(chunk.get(..n).unwrap_or_default());
        if kept.len() > MAX_BODY_BYTES {
            return Ok((kept, true));
        }
    }
}

#[async_trait]
impl SynthBackend for External {
    /// Sends `describe` (R-SYNTH-26): the backend's name and version, each 1..=128 cleaned characters; the model of the
    /// identity is `<backend>@<backend_version>`, as Ollama's is `<model>@<digest>`.
    async fn identify(&self) -> Result<Identity, ProviderError> {
        let (doc, tail) = self.exchange(&Call::Describe).await?;
        // The token is taken out before the text is cleaned and kept, so a name that is the token is `***` (D-102).
        let text = |key: &str| {
            doc.get(key)
                .and_then(Value::as_str)
                .and_then(|text| clean_name(&self.redact(text), MAX_NAME_CHARS))
        };
        let (Some(backend), Some(version)) = (text("backend"), text("backend_version")) else {
            return Err(failed(
                "its `describe` reply wasn't a `backend` and a `backend_version` of 1 to 128 characters each, with no control or invisible characters",
                &tail,
            ));
        };
        Ok(Identity {
            provider: PROVIDER.to_owned(),
            model: format!("{backend}@{version}"),
            input_version: REQUEST_VERSION.to_owned(),
            backend: Some(backend),
        })
    }

    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(ExternalProvider {
            backend: self.clone(),
            identity: identity.clone(),
        }))
    }
}

/// The provider an [`External`] opens.
struct ExternalProvider {
    backend: External,
    identity: Identity,
}

#[async_trait]
impl SynthProvider for ExternalProvider {
    fn id(&self) -> &str {
        PROVIDER
    }

    fn model(&self) -> &str {
        &self.identity.model
    }

    fn input_version(&self) -> &str {
        &self.identity.input_version
    }

    fn backend(&self) -> &str {
        self.identity.backend.as_deref().unwrap_or(PROVIDER)
    }

    /// One `synthesize` request (R-SYNTH-26): one provider call; the transport tries inside it wait as every provider's do
    /// (R-SYNTH-12), and a reply the service gave is never asked for again by the transport (R-SYNTH-28).
    async fn complete(&self, request: &SynthRequest, _limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let started = Instant::now();
        let body = to_canonical_string(request)
            .map_err(|_| ProviderError::Internal("the request could not be written".to_owned()))?;
        let (doc, tail) = self.backend.exchange(&Call::Synthesize(&body)).await?;
        let reply = self.backend.read_reply(doc, &tail)?;
        Ok(SynthReply {
            reply_json: reply,
            usage: Usage::default(),
            latency: started.elapsed(),
        })
    }
}

impl External {
    /// The reply document of a `synthesize` request: exactly one of `body`, `question`, `pending` or `error` (R-SYNTH-28),
    /// as the reply text the retry loop reads (R-SYNTH-10). Any other key is ignored (R-SYNTH-26). The token is taken out
    /// of message texts; a `body` that holds it is refused, never rewritten (D-102, D-103).
    fn read_reply(&self, mut doc: Map<String, Value>, tail: &str) -> Result<String, ProviderError> {
        let unknown = || {
            failed(
                "its reply wasn't exactly one of `body`, `question`, `pending` or `error`",
                tail,
            )
        };
        let kinds: Vec<&str> = ["body", "question", "pending", "error"]
            .into_iter()
            .filter(|kind| doc.contains_key(*kind))
            .collect();
        let (&[kind], Some(value)) = (kinds.as_slice(), kinds.first().and_then(|kind| doc.remove(*kind))) else {
            return Err(unknown());
        };
        let text = |value: Value| {
            value
                .as_str()
                .map(|text| self.redact(text))
                .ok_or_else(|| failed("a reply text wasn't a string", tail))
        };
        match (kind, value) {
            ("body", body @ Value::Object(_)) => {
                let canonical = to_canonical_string(&serde_json::json!({ "body": body })).map_err(|_| unknown())?;
                if self.holds_token(&canonical) {
                    return Err(failed("the reply contains your token", tail));
                }
                Ok(canonical)
            }
            ("question", question) => {
                let question = text(question)?;
                to_canonical_string(&serde_json::json!({ "question": question })).map_err(|_| unknown())
            }
            ("pending", pending) => Err(ProviderError::Pending(text(pending)?)),
            ("error", reason) => Err(failed(
                &format!("it reported an error: {}", clean_line(&text(reason)?)),
                tail,
            )),
            _ => Err(unknown()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExternalToken, ExternalUrl, TokenMalformed, UrlError, localhost_addrs};

    /// Plain `http` only for this machine; every other host needs `https`; userinfo, other schemes, queries and junk are
    /// refused before any contact (R-SYNTH-29, D-101).
    #[test]
    fn a_url_is_https_or_plain_http_to_this_machine_only() {
        for good in [
            "http://localhost:8080",
            "http://LOCALHOST",
            "http://127.0.0.1:9/x/",
            "http://127.9.8.7",
            "http://[::1]:8080",
            "https://example.com",
            "HTTPS://svc.example.com:8443/base",
            "https://svc.example.com/a-b_c.d~e/%41/x:y@z/(a);b=c,d",
        ] {
            assert!(ExternalUrl::parse(good).is_ok(), "{good}");
        }
        for (bad, why) in [
            ("http://example.com", UrlError::Insecure),
            ("http://128.0.0.1", UrlError::Insecure),
            ("http://127.0.0.1.evil.example", UrlError::Insecure),
            ("http://localhost.evil.example", UrlError::Insecure),
            ("http://[::2]", UrlError::Insecure),
            ("http://0.0.0.0", UrlError::Insecure),
            ("ftp://localhost", UrlError::Scheme),
            ("file:///etc/passwd", UrlError::Scheme),
            ("https://user:pw@example.com", UrlError::Userinfo),
            ("http://user@localhost", UrlError::Userinfo),
            ("localhost:8080", UrlError::Unparsable),
            ("example.com", UrlError::Unparsable),
            ("https://", UrlError::Unparsable),
            ("https://exa mple.com", UrlError::Unparsable),
            ("https://example.com:99999", UrlError::Unparsable),
            ("https://example.com:", UrlError::Unparsable),
            ("https://example.com/x?y=1", UrlError::Unparsable),
            ("https://example.com/#x", UrlError::Unparsable),
            ("https://example.com/a\"b", UrlError::Unparsable),
            ("https://example.com/<x>", UrlError::Unparsable),
            ("https://example.com/{x}", UrlError::Unparsable),
            ("https://example.com/a\\b", UrlError::Unparsable),
            ("https://example.com/%zz", UrlError::Unparsable),
            ("https://example.com/%4", UrlError::Unparsable),
            ("https://[::1", UrlError::Unparsable),
            ("https://ex\u{e9}mple.com", UrlError::Unparsable),
            ("", UrlError::Unparsable),
        ] {
            assert_eq!(ExternalUrl::parse(bad), Err(why), "{bad}");
        }
        let url = ExternalUrl::parse(" https://svc.example.com:8443/base/ ").expect("a URL");
        assert_eq!(url.host(), "svc.example.com");
        assert_eq!(
            url.endpoint("/v1/describe"),
            "https://svc.example.com:8443/base/v1/describe"
        );
        assert_eq!(ExternalUrl::parse("http://[::1]:80").expect("a URL").host(), "[::1]");
    }

    /// A token is visible ASCII once trimmed; empty is none; anything else is an error, and it never prints (R-SEC-06).
    #[test]
    fn a_token_is_visible_ascii_and_never_printed() {
        assert_eq!(ExternalToken::parse(""), Ok(None));
        assert_eq!(ExternalToken::parse("  "), Ok(None));
        let token = ExternalToken::parse(" s3cret-token-0123456 ")
            .expect("a token")
            .expect("one");
        assert_eq!(token.0, "s3cret-token-0123456");
        assert!(!format!("{token:?} {token}").contains("s3cret-token-0123456"));
        for bad in [
            "a b-0123456789abcdef",
            "a\nb-0123456789abcdef",
            "a\u{1b}b-0123456789abcdef",
            "t\u{e9}-0123456789abcdef",
            "a\u{0}-0123456789abcdef",
            // Shorter than 16 characters.
            "s3cret",
            "0123456789abcde",
        ] {
            assert_eq!(ExternalToken::parse(bad), Err(TokenMalformed), "{bad:?}");
        }
    }

    /// `localhost` is 127.0.0.1 and `::1`, with the URL's port, and nothing DNS says (R-SYNTH-29, D-102).
    #[test]
    fn localhost_is_the_loopback_addresses() {
        let addrs = localhost_addrs(8080);
        assert_eq!(addrs.map(|a| a.to_string()), ["127.0.0.1:8080", "[::1]:8080"]);
        assert!(addrs.iter().all(|a| a.ip().is_loopback()));
    }
}
