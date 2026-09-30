//! The base URL of a service Velme talks to that the user names (`external` and `ollama`), checked before any contact, and the
//! resolver that keeps `localhost` on this machine (`compiler/22` R-SYNTH-29, D-101, D-102, D-111).

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use ureq::config::Config as UreqConfig;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;

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

    /// Whether the URL names this machine: `localhost`, or a loopback address in any spelling. Such a server is reached
    /// directly, never through a proxy from the environment (D-111).
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    /// The URL of the endpoint `path`, which starts with `/`.
    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.text)
    }
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
pub(crate) fn localhost_addrs(port: u16) -> [SocketAddr; 2] {
    [
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
    ]
}

/// Resolves `localhost` to [`localhost_addrs`] and every other host as usual.
#[derive(Debug)]
pub(crate) struct LocalResolver;

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
