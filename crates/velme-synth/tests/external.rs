//! The `external` provider against the test backend and a mock server (`compiler/22` §3.2, R-SYNTH-26..29, R-SYNTH-41, D-42,
//! D-45, D-98, D-101): a real HTTP service on 127.0.0.1, with the backend's own misbehaviour on request. No wall-clock wait
//! but the timeout tests, which use a short timeout and an injected sleeper.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use velme_synth::{
    External, ExternalConfig, ExternalToken, ExternalUrl, Identity, ProviderError, REQUEST_VERSION, SynthBackend,
    SynthLimits, SynthRequest, build_request,
};
use velme_test_support::backend::{Config, Mode, On, Server};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{RecordingSleeper, goal_id, program, repo};

const SOURCE: &str = "language: velme/0.1

type Player:
    name: Text
    score: Number

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise. SECRET-PLAN-TEXT
    check:
        - result == \"Gold\" or result == \"Silver\" or result == \"Bronze\"
    examples:
        - FindBadge(Player(name: \"Lina\", score: 820)) == \"Silver\"
";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
}

fn request() -> SynthRequest {
    let program = program(SOURCE);
    build_request(&program, goal_id(&program, "FindBadge"), SOURCE).expect("a request")
}

fn limits() -> SynthLimits {
    SynthLimits::new(velme_ir::Fingerprint::of_bytes(b"goal"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("external").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

const PATIENT: Duration = Duration::from_secs(30);

/// The backend at `url`, with a timeout of `timeout` and a sleeper that never waits.
fn at(url: &str, token: Option<&str>, timeout: Duration) -> (External, RecordingSleeper) {
    let mut config = ExternalConfig::new(ExternalUrl::parse(url).expect("a URL"));
    config.token = token.map(|t| ExternalToken::parse(t).expect("a token").expect("one"));
    config.timeout = timeout;
    let sleeper = RecordingSleeper::default();
    (External::new(config).with_sleeper(Arc::new(sleeper.clone())), sleeper)
}

/// The test backend for `config`, and a client of it.
fn serving(config: Config) -> (Server, External) {
    let server = Server::start(config);
    let (backend, _) = at(server.url(), None, PATIENT);
    (server, backend)
}

/// A backend that answers `reply` (a JSON text) to every `FindBadge`, from a directory holding that one file.
fn replying(name: &str, reply: &str) -> (Server, External) {
    let dir = scratch(name);
    fs::write(dir.join("FindBadge.json"), reply).expect("reply file");
    serving(Config::replying(dir))
}

fn complete(backend: &External, request: &SynthRequest) -> Result<String, ProviderError> {
    let identity = Identity {
        provider: "external".to_owned(),
        model: "v".to_owned(),
        input_version: REQUEST_VERSION.to_owned(),
        backend: Some("test".to_owned()),
    };
    let provider = backend.open(&identity).expect("a provider");
    block_on(provider.complete(request, &limits())).map(|reply| reply.reply_json)
}

fn failure(result: Result<String, ProviderError>) -> String {
    match result.expect_err("a failure") {
        ProviderError::BackendFailed { reason, body } if body.is_empty() => reason,
        ProviderError::BackendFailed { reason, body } => format!("{reason}\n{body}"),
        other => panic!("not a backend failure: {other:?}"),
    }
}

/// `describe` names the backend and its version, which are the model and the request version of the identity
/// (R-SYNTH-25, R-SYNTH-26).
#[test]
fn describe_gives_the_backend_name_and_version() {
    let (_server, backend) = serving(Config::default().describing("  my   backend ", "v  2"));
    let identity = block_on(backend.identify()).expect("identity");
    assert_eq!(identity.provider, "external");
    assert_eq!(identity.backend.as_deref(), Some("my backend"));
    assert_eq!(identity.model, "my backend@v 2");
    assert_eq!(identity.input_version, REQUEST_VERSION);
}

/// A name or version that is empty, over 128 characters, blank, or holding a control, format or bidi character, or a
/// reply with anything else in it, is a backend failure (R-SYNTH-26, D-98).
#[test]
fn a_describe_reply_that_breaks_the_rules_is_a_backend_failure() {
    for (name, version) in [
        ("", "v"),
        ("n", ""),
        (" \u{7} ", "v"),
        // Control, format and bidi characters are refused, not cleaned away (R-SYNTH-26, D-47).
        ("my\u{1b}[1m backend", "v"),
        ("n", "v\n2"),
        ("n\u{202e}", "v"),
        ("n", "v\u{200b}2"),
        ("\u{feff}n", "v"),
        (&"n".repeat(129), "v"),
        ("n", &"v".repeat(129)),
    ] {
        let (_server, backend) = serving(Config::default().describing(name, version));
        let error = block_on(backend.identify()).expect_err("not a description");
        assert!(
            matches!(&error, ProviderError::BackendFailed { .. }),
            "{name:?} {version:?}: {error:?}"
        );
    }
    let (_server, edge) = serving(Config::default().describing(&"n".repeat(128), &"v".repeat(128)));
    assert!(block_on(edge.identify()).is_ok(), "128 characters are allowed");
    // A `synthesize`-shaped answer, garbage and a wrong-typed field.
    for body in ["{}", "not json", r#"{"backend": 1, "backend_version": "v"}"#, "[1]"] {
        let server = MockServer::start([MockResponse::ok(body)]);
        let (backend, _) = at(server.url(), None, PATIENT);
        let error = block_on(backend.identify()).expect_err("not a description");
        assert!(
            matches!(error, ProviderError::BackendFailed { .. }),
            "{body}: {error:?}"
        );
    }
}

/// A `describe` reply may carry keys Velme doesn't know; only the two fields matter (R-SYNTH-26).
#[test]
fn unknown_describe_keys_are_ignored() {
    let mut config = Config::default().describing("queue", "v1");
    config.describe_extra = true;
    let (_server, backend) = serving(config);
    let identity = block_on(backend.identify()).expect("identity");
    assert_eq!(identity.backend.as_deref(), Some("queue"));
}

/// The text of an `{"error"}` reply is cleaned, collapsed and bounded like any other untrusted line (R-SYNTH-28).
#[test]
fn an_error_reply_is_cleaned_and_bounded() {
    let text = format!("bad\u{1b}[31m\n\tthing {}", "x".repeat(1000));
    let (_server, backend) = replying("error-text", &json!({"error": text}).to_string());
    let error = complete(&backend, &request()).expect_err("a failure");
    let ProviderError::BackendFailed { reason, body } = error else {
        panic!("not a backend failure");
    };
    assert!(!body.contains(['\u{1b}', '\n', '\t']));
    assert!(reason.starts_with("it reported an error: bad thing xxx"), "{reason}");
    assert!(!reason.contains(['\u{1b}', '\n', '\t']));
    assert!(reason.chars().count() < 340, "{}", reason.chars().count());
}

/// A `body` reply is the body under `body`; a `question` reply is the question object; `pending` keeps its text; an `error` reply
/// and a reply of no known kind are backend failures (R-SYNTH-27, R-SYNTH-28, R-SYNTH-41).
#[test]
fn a_reply_is_one_of_four_kinds() {
    let body = json!({"kind": "literal", "type": {"t": "Number"}, "value": 1.50});
    let (_server, backend) = replying("kind-body", &json!({"body": body, "note": 1.50}).to_string());
    assert_eq!(
        complete(&backend, &request()).expect("a body reply"),
        r#"{"body":{"kind":"literal","type":{"t":"Number"},"value":1.5}}"#
    );
    let (_server, backend) = replying("kind-question", r#"{"question": "Which way?"}"#);
    assert_eq!(
        complete(&backend, &request()).expect("a question"),
        r#"{"question":"Which way?"}"#
    );
    let (_server, backend) = replying("kind-pending", r#"{"pending": "ticket 42"}"#);
    assert_eq!(
        complete(&backend, &request()),
        Err(ProviderError::Pending("ticket 42".to_owned()))
    );
    let (_server, backend) = replying("kind-error", r#"{"error": "no idea"}"#);
    assert!(failure(complete(&backend, &request())).contains("no idea"));
    for (name, reply) in [
        ("kind-empty", "{}"),
        ("kind-two", r#"{"body": {}, "question": "?"}"#),
        ("kind-unknown", r#"{"result": 1}"#),
        ("kind-body-string", r#"{"body": "text"}"#),
        ("kind-pending-number", r#"{"pending": 7}"#),
        ("kind-array", "[1]"),
    ] {
        let (_server, backend) = replying(name, reply);
        let text = failure(complete(&backend, &request()));
        assert!(!text.is_empty(), "{name}");
    }
}

/// A reply that isn't JSON, one past 2 MiB and a server error that outlives the transport retries are backend failures
/// whose notes hold the last words of the reply body, cleaned; nothing is asked twice but a server error (AC-SYNTH-16,
/// R-SYNTH-28).
#[test]
fn ac_synth_16_garbage_a_flood_and_a_server_error_are_backend_failures_with_the_tail_of_the_body() {
    let dir = scratch("failures");
    let garbage = Config::replying(&dir).misbehaving(Mode::Garbage, On::Both);
    let (_server, backend) = serving(garbage);
    let text = failure(complete(&backend, &request()));
    assert!(text.starts_with("its reply wasn't one JSON object"), "{text}");
    assert!(text.ends_with("this is not JSON"), "{text}");
    let described = block_on(backend.identify());
    assert!(
        matches!(&described, Err(ProviderError::BackendFailed { body, .. }) if body.contains("not JSON")),
        "{described:?}"
    );
    let (_server, flood) = serving(Config::replying(&dir).misbehaving(Mode::Huge, On::Synthesize));
    let flood = failure(complete(&flood, &request()));
    assert!(flood.starts_with("it wrote more than 2 MiB"), "{flood}");

    // A `5xx` is retried as a transport failure, then it is the backend failing, with its body as the notes.
    let server = Server::start(Config::replying(&dir).misbehaving(Mode::Status(500), On::Synthesize));
    let (backend, sleeper) = at(server.url(), None, PATIENT);
    let text = failure(complete(&backend, &request()));
    assert_eq!(text, "it answered with status 500\n{\"note\":\"a test failure\"}");
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);

    // Any other status is the backend's answer and is not asked again.
    let mock = MockServer::start([MockResponse::status(404, "no such thing\u{1b}[31m here")]);
    let (backend, sleeper) = at(mock.url(), None, PATIENT);
    assert_eq!(
        failure(complete(&backend, &request())),
        "it answered with status 404\nno such thing here"
    );
    assert!(sleeper.waits().is_empty());
    assert_eq!(mock.requests().len(), 1);
}

/// A body past 2 MiB is a failure however small the rest of the reply is (R-SYNTH-28).
#[test]
fn an_oversized_body_is_a_backend_failure() {
    let flood = format!(r#"{{"question": "{}"}}"#, "x".repeat(3 * 1024 * 1024));
    let server = MockServer::start([MockResponse::ok(flood)]);
    let (backend, _) = at(server.url(), None, PATIENT);
    let text = failure(complete(&backend, &request()));
    assert!(text.starts_with("it wrote more than 2 MiB"), "{text}");
}

/// A request that outlives the timeout is `Timeout`, `describe` included, after the transport retries, and the wait is not
/// the backend's to hold up (R-SYNTH-28, D-98, D-101).
#[test]
fn a_backend_that_outlives_the_timeout_is_a_timeout() {
    let dir = scratch("timeout");
    let quick = Duration::from_millis(300);
    let server = Server::start(Config::replying(&dir).misbehaving(Mode::Hang, On::Both));
    let (backend, sleeper) = at(server.url(), None, quick);
    assert_eq!(complete(&backend, &request()), Err(ProviderError::Timeout));
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);
    assert!(matches!(block_on(backend.identify()), Err(ProviderError::Timeout)));
}

/// A service that isn't there is `Unavailable` after the transport retries, `describe` included (R-SYNTH-12, D-101).
#[test]
fn a_service_that_is_not_there_is_unavailable() {
    let gone = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        format!("http://{}", listener.local_addr().expect("an address"))
    };
    let (backend, sleeper) = at(&gone, None, PATIENT);
    assert!(matches!(
        complete(&backend, &request()),
        Err(ProviderError::Unavailable(_))
    ));
    assert_eq!(sleeper.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);
    assert!(matches!(
        block_on(backend.identify()),
        Err(ProviderError::Unavailable(_))
    ));
}

/// A redirect is never followed, and so nothing, the token included, goes to where it points (`tooling/41` R-SEC-13, D-101).
#[test]
fn a_redirect_is_not_followed() {
    let dir = scratch("redirect");
    let log = dir.join("log.txt");
    let server = Server::start(
        Config::replying(&dir)
            .logging(&log)
            .misbehaving(Mode::Redirect, On::Both),
    );
    let (backend, _) = at(server.url(), Some("tok-redirect-0123456"), PATIENT);
    let text = failure(complete(&backend, &request()));
    assert!(text.contains("redirect"), "{text}");
    assert!(matches!(
        block_on(backend.identify()),
        Err(ProviderError::BackendFailed { .. })
    ));
    assert_eq!(
        fs::read_to_string(&log).expect("log"),
        "synthesize FindBadge\ndescribe\n",
        "the redirect target was asked"
    );
    // Another host that the redirect names gets nothing.
    let elsewhere = MockServer::start([MockResponse::ok(r#"{"question": "followed"}"#)]);
    let target = format!("{}/v1/synthesize", elsewhere.url());
    let mock = MockServer::start([MockResponse::status(307, "").header("location", &target)]);
    let (backend, _) = at(mock.url(), Some("tok-redirect-0123456"), PATIENT);
    assert!(failure(complete(&backend, &request())).contains("redirect"));
    assert!(elsewhere.requests().is_empty(), "the redirect was followed");
}

/// The token is sent as a bearer token to the configured URL on every request, and a `401` or `403` is `TokenRejected`,
/// never retried; a service that echoes the token in a failure doesn't get it into the notes (R-SEC-13, D-101).
#[test]
fn the_token_is_sent_as_a_bearer_and_a_rejection_is_named() {
    let (_server, plain) = serving(Config::default().wanting_token("tok-123-0123456789"));
    assert!(matches!(
        block_on(plain.identify()),
        Err(ProviderError::TokenRejected { sent: false })
    ));
    let server = Server::start(Config::default().wanting_token("tok-123-0123456789"));
    let (wrong, sleeper) = at(server.url(), Some("tok-999-0123456789"), PATIENT);
    assert!(matches!(
        block_on(wrong.identify()),
        Err(ProviderError::TokenRejected { sent: true })
    ));
    assert!(sleeper.waits().is_empty(), "a rejection is not retried");
    let (right, _) = at(server.url(), Some("tok-123-0123456789"), PATIENT);
    assert!(block_on(right.identify()).is_ok());

    for status in [401, 403] {
        let mock = MockServer::start([
            MockResponse::status(status, "denied"),
            MockResponse::status(status, "denied"),
        ]);
        let (backend, _) = at(mock.url(), Some("tok-123-0123456789"), PATIENT);
        assert!(matches!(
            block_on(backend.identify()),
            Err(ProviderError::TokenRejected { sent: true })
        ));
        assert!(matches!(
            complete(&backend, &request()),
            Err(ProviderError::TokenRejected { sent: true })
        ));
        let sent = mock.requests();
        assert_eq!(sent[0].headers["authorization"], "Bearer tok-123-0123456789");
        assert_eq!(sent[0].method, "GET");
        assert_eq!(sent[0].path, "/v1/describe");
        assert_eq!(sent[1].method, "POST");
        assert_eq!(sent[1].path, "/v1/synthesize");
        assert_eq!(sent[1].headers["authorization"], "Bearer tok-123-0123456789");
    }
    // Without a token there is no header at all.
    let mock = MockServer::start([MockResponse::ok(r#"{"backend": "b", "backend_version": "v"}"#)]);
    let (backend, _) = at(mock.url(), None, PATIENT);
    block_on(backend.identify()).expect("identity");
    assert!(!mock.requests()[0].headers.contains_key("authorization"));

    // A service that echoes the token back doesn't get it into a diagnostic.
    let mock = MockServer::start([MockResponse::status(
        400,
        "bad request; you sent tok-123-0123456789 and more",
    )]);
    let (backend, _) = at(mock.url(), Some("tok-123-0123456789"), PATIENT);
    let text = failure(complete(&backend, &request()));
    assert!(text.contains("you sent *** and more"), "{text}");
    assert!(!text.contains("tok-123-0123456789"));
}

/// The `synthesize` request body is the canonical request itself: it is the snapshot, parses back through the committed
/// types of the request schema, and holds no file path, environment value or key (AC-SYNTH-14, R-SYNTH-26).
#[test]
fn ac_synth_14_the_synthesize_body_is_the_snapshot_and_holds_nothing_local() {
    let dir = scratch("message");
    let capture = dir.join("body.json");
    let (_server, backend) = serving(Config::replying(&dir).capturing(&capture));
    // The backend has no reply for the goal, so this is an `error`; the body it received is what is checked.
    let _ = complete(&backend, &request());
    let text = fs::read_to_string(&capture).expect("the body the backend received");
    let body: SynthRequest = velme_ir::from_json_str(&text).expect("the committed request types accept it");
    assert_eq!(body, request());
    assert_eq!(text, velme_ir::to_canonical_string(&request()).expect("canonical"));
    let mut shown = serde_json::to_value(&body).expect("serializes");
    assert_eq!(shown["request_version"], REQUEST_VERSION);
    shown["output_schema"] = json!("<reply schema>");
    insta::assert_snapshot!(
        "external_synthesize_message",
        serde_json::to_string_pretty(&shown).expect("prints")
    );
    let mut forbidden = vec![
        dir.to_string_lossy().into_owned(),
        repo("").to_string_lossy().into_owned(),
        env!("CARGO_MANIFEST_DIR").to_owned(),
        "sk-".to_owned(),
        "API_KEY".to_owned(),
    ];
    forbidden.extend(
        ["HOME", "USER", "PATH"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .filter(|v| v.len() > 3),
    );
    for value in forbidden {
        assert!(!text.contains(&value), "the body holds {value}");
    }
}

/// Nothing the service says can carry the token into a `ProviderError`: the `{"error"}` reason, the pending and question
/// text, the describe fields and the IR, in any spelling, and a token cut by the start of the 4 KiB tail (R-SEC-13, D-101).
#[test]
fn the_token_is_taken_out_of_everything_the_service_says() {
    let token = "tok-123-01234567894abcd";
    let text = |result: Result<String, ProviderError>| format!("{result:?}");
    for (name, reply) in [
        ("error", json!({"error": format!("bad token {token}")})),
        ("pending", json!({"pending": format!("ticket {token}")})),
        ("question", json!({"question": format!("is it {token}?")})),
    ] {
        let server = MockServer::start([MockResponse::ok(reply.to_string())]);
        let (backend, _) = at(server.url(), Some(token), PATIENT);
        let shown = text(complete(&backend, &request()));
        assert!(!shown.contains(token), "{name}: {shown}");
        assert!(shown.contains("***"), "{name}: {shown}");
    }
    // A `body` is never rewritten: one that holds the token, in either spelling, is refused (D-102).
    let odd = "a/b\"c-0123456789abc";
    for (spelling, token) in [("plain", token), ("escaped", odd)] {
        let body = json!({"body": {"kind": "literal", "type": {"t": "Text"}, "value": format!("x {token} y")}});
        let server = MockServer::start([MockResponse::ok(body.to_string())]);
        let (backend, _) = at(server.url(), Some(token), PATIENT);
        let shown = failure(complete(&backend, &request()));
        assert!(
            shown.starts_with("the reply contains your token"),
            "{spelling}: {shown}"
        );
        assert!(!shown.contains(token), "{spelling}: {shown}");
    }
    // Describe: the name and the version, which become the identity and the manifest.
    let server = MockServer::start([MockResponse::ok(
        json!({"backend": token, "backend_version": token}).to_string(),
    )]);
    let (backend, _) = at(server.url(), Some(token), PATIENT);
    let identity = block_on(backend.identify()).expect("identity");
    assert!(!format!("{identity:?}").contains(token), "{identity:?}");
    // A token that JSON spells with an escape is found too.
    let server = MockServer::start([MockResponse::ok(json!({"error": format!("x {odd} y")}).to_string())]);
    let (backend, _) = at(server.url(), Some(odd), PATIENT);
    let shown = text(complete(&backend, &request()));
    assert!(!shown.contains(odd) && !shown.contains("a\\/b"), "{shown}");

    // The token straddles the cut of the 4 KiB tail: redacting comes first, so no half of it is left.
    let body = format!("{}{token}{}", "x".repeat(100), "y".repeat(4096 - 4));
    let server = MockServer::start([MockResponse::status(400, body)]);
    let (backend, _) = at(server.url(), Some(token), PATIENT);
    let shown = failure(complete(&backend, &request()));
    assert!(
        !shown.contains("1234abcd-0123456") && !shown.contains("abcd-0123456") && !shown.contains(token),
        "{shown}"
    );
    assert!(shown.contains("***"), "{shown}");
}

/// A body that stalls after the headers were sent is a `Timeout`, not an unavailable service (R-SYNTH-28).
#[test]
fn a_body_that_stalls_is_a_timeout() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let thread = std::thread::spawn(move || {
        // Three tries, each answered with headers and the first byte of a longer body, then silence.
        let mut held = Vec::new();
        for stream in listener.incoming().take(3) {
            let mut stream = stream.expect("a connection");
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{")
                .expect("headers");
            held.push(stream);
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    let (backend, sleeper) = at(&url, None, Duration::from_millis(300));
    assert_eq!(complete(&backend, &request()), Err(ProviderError::Timeout));
    assert_eq!(sleeper.waits().len(), 2);
    thread.join().expect("the server thread");
}

/// The identity's model is `<backend>@<backend_version>`, like Ollama's `<model>@<digest>` (R-SYNTH-26, D-102).
#[test]
fn the_model_is_the_backend_at_its_version() {
    let (_server, backend) = serving(Config::default().describing("queue", "v7"));
    let identity = block_on(backend.identify()).expect("identity");
    assert_eq!(identity.model, "queue@v7");
    assert_eq!(identity.backend.as_deref(), Some("queue"));
}

/// A reply with exactly one of the four kinds is read whatever other keys it has; none, or two, is a failure
/// (R-SYNTH-26, R-SYNTH-28, D-102).
#[test]
fn a_reply_may_carry_extra_keys() {
    let (_server, backend) = replying(
        "extra-keys",
        r#"{"question": "Which way?", "trace_id": "t-1", "meta": {"a": 1}}"#,
    );
    assert_eq!(
        complete(&backend, &request()).expect("a question"),
        r#"{"question":"Which way?"}"#
    );
    let (_server, backend) = replying("extra-key", r#"{"trace_id": 7, "pending": "ticket 42"}"#);
    assert_eq!(
        complete(&backend, &request()),
        Err(ProviderError::Pending("ticket 42".to_owned()))
    );
    for (name, reply) in [
        ("extra-none", r#"{"trace_id": "t-1"}"#),
        ("extra-two", r#"{"question": "?", "pending": "p", "trace_id": "t"}"#),
    ] {
        let (_server, backend) = replying(name, reply);
        assert!(
            failure(complete(&backend, &request())).contains("exactly one"),
            "{name}"
        );
    }
}
