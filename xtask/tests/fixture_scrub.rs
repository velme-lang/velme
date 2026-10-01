//! The fixture scrub (`delivery/51` §4, `tooling/41` R-SEC-07): no file under `tests/fixtures` holds a key-shaped
//! string or a request header.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use xtask::workspace_root;

/// Prefixes of provider and hosting keys; a word starting with one and at least [`KEY_LEN`] long is key-shaped.
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "AKIA",
    "AIza",
    "xoxb-",
    "xoxp-",
];
const KEY_LEN: usize = 20;

/// Request header names a recording strips (R-SEC-07), lower-cased; one counts only in header form, the name followed
/// by `:` or, quoted, by `":`.
const HEADERS: &[&str] = &["x-api-key", "authorization"];

/// The key-shaped words, request headers and bearer tokens in `text`.
fn key_shaped(text: &str) -> Vec<String> {
    let mut found: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|word| word.len() >= KEY_LEN && KEY_PREFIXES.iter().any(|p| word.starts_with(p)))
        .map(str::to_owned)
        .collect();
    let lower = text.to_ascii_lowercase();
    for header in HEADERS {
        for (at, _) in lower.match_indices(header) {
            let before = lower[..at].chars().next_back();
            let after = &lower[at + header.len()..];
            let named = !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if named && (after.starts_with(':') || after.starts_with("\":")) {
                found.push(format!("{header}:"));
            }
        }
    }
    // `bearer ` and then a token: a word of at least 16 token characters (R-SEC-13's shortest token).
    for (at, _) in lower.match_indices("bearer ") {
        let token: String = text[at + "bearer ".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || "-._~+/=".contains(*c))
            .collect();
        if token.len() >= 16 {
            found.push(format!("bearer {token}"));
        }
    }
    found
}

/// Every file under `dir`, sorted.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).expect("a fixture directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                all.push(path);
            }
        }
    }
    all.sort();
    all
}

#[test]
fn r_sec_07_fixtures_hold_no_key_shaped_string() {
    let fixtures = workspace_root().expect("the repository root").join("tests/fixtures");
    let files = files(&fixtures);
    assert!(!files.is_empty(), "no fixtures under {}", fixtures.display());
    let offending: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = String::from_utf8_lossy(&fs::read(path).expect("a fixture")).into_owned();
            key_shaped(&text)
                .into_iter()
                .map(move |what| format!("{}: {what}", path.display()))
        })
        .collect();
    assert!(offending.is_empty(), "{offending:#?}");
}

/// The scrub is not vacuous: each kind of key it looks for, and each header, is found.
#[test]
fn r_sec_07_the_scrub_finds_keys_and_headers() {
    let key = format!("{}{}", "sk-ant-", "a".repeat(KEY_LEN));
    assert_eq!(key_shaped(&format!("{{\"reply\":\"{key}\"}}")), [key]);
    for prefix in KEY_PREFIXES {
        let word = format!("{prefix}{}", "0".repeat(KEY_LEN));
        assert_eq!(key_shaped(&word), [word]);
    }
    assert_eq!(key_shaped("\"X-Api-Key\": \"\""), ["x-api-key:"]);
    assert_eq!(key_shaped("x-api-key: k"), ["x-api-key:"]);
    assert_eq!(key_shaped("{\"authorization\":\"\"}"), ["authorization:"]);
    let token = "abcdefgh12345678";
    assert_eq!(
        key_shaped(&format!("Authorization: Bearer {token}")),
        ["authorization:".to_owned(), format!("bearer {token}")]
    );
    // Header names and `bearer` in prose or in a name are not headers.
    assert!(key_shaped("goal CheckAuthorization(x: Number) -> Number: the authorization step").is_empty());
    assert!(key_shaped("Send it as a bearer token, never an x-api-key query.").is_empty());
    // Hashes and short words are not keys.
    assert!(key_shaped("b3-065dc4431759128d124e02af97abcad9a02cf551cadf6c261e65a96e4d57ffe2 sk-short").is_empty());
}
