//! The test `external` backend (`compiler/22` §3.2, D-42, D-99, D-101): a tiny deterministic HTTP service that answers
//! the protocol from hand-written goal bodies and misbehaves on request, so the tests of the `external` provider and the
//! unattended recording of the examples' replay fixtures need no live provider. It uses `std` networking on 127.0.0.1 and
//! no async runtime, so it runs on every OS. Not a product: nothing here is reachable from `velme`.
//!
//! `GET /v1/describe` answers `{"backend", "backend_version"}`. `POST /v1/synthesize` for goal `G` answers with
//! `DIR/G.json` (`DIR/G.N.json` when the request carries `N` earlier attempts, else `G.json`): a file holding a body
//! (an expression node, so it has a `kind`) is wrapped as `{"body": …}`, any other file is sent as it is. A goal with no file is an `{"error"}` reply.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use crate::mock::read_request;

/// How the backend misbehaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Answers with this status and a short JSON body: `500`, `401`, `404`, …
    Status(u16),
    /// Answers `200` with a body that is not JSON.
    Garbage,
    /// Answers `200` with a body that is not JSON and holds an OSC 52 clipboard sequence, a right-to-left override and a C1
    /// control character: what a terminal must never be sent raw (AC-CLI-14).
    Hostile,
    /// Answers `200` with a body of more than 2 MiB.
    Huge,
    /// Accepts the request and never answers, until the client gives up.
    Hang,
    /// Answers `302` to `/v1/elsewhere`, which counts as a hit in the log if it is followed.
    Redirect,
}

/// Which request a [`Mode`] applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum On {
    /// `GET /v1/describe`.
    Describe,
    /// `POST /v1/synthesize`.
    Synthesize,
    /// Both.
    Both,
}

/// What the backend does.
#[derive(Debug, Clone)]
pub struct Config {
    /// The directory of replies, one file per goal.
    pub dir: Option<PathBuf>,
    /// A file that gets one line per request: `describe`, `synthesize <Goal>` or `unknown <path>`.
    pub log: Option<PathBuf>,
    /// A file that holds the body of the last `synthesize` request.
    pub capture: Option<PathBuf>,
    /// A file that holds the `Authorization` header of the last request, if it had one.
    pub auth_dump: Option<PathBuf>,
    /// The name and version `describe` answers.
    pub describe: (String, String),
    /// Whether `describe` also carries a key Velme doesn't know.
    pub describe_extra: bool,
    /// The bearer token every request must carry; a request without it is `401`.
    pub token: Option<String>,
    /// How to misbehave, and on which request.
    pub mode: Option<(Mode, On)>,
    /// The port to listen on; 0 for any free one.
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            dir: None,
            log: None,
            capture: None,
            auth_dump: None,
            describe: ("velme-test-support".to_owned(), "hand-written".to_owned()),
            describe_extra: false,
            token: None,
            mode: None,
            port: 0,
        }
    }
}

impl Config {
    /// A backend that answers from the replies in `dir`.
    pub fn replying(dir: impl Into<PathBuf>) -> Self {
        Config {
            dir: Some(dir.into()),
            ..Config::default()
        }
    }

    /// The backend with a log file.
    #[must_use]
    pub fn logging(mut self, log: impl Into<PathBuf>) -> Self {
        self.log = Some(log.into());
        self
    }

    /// The backend with a capture file.
    #[must_use]
    pub fn capturing(mut self, capture: impl Into<PathBuf>) -> Self {
        self.capture = Some(capture.into());
        self
    }

    /// The backend that misbehaves as `mode` on `on`.
    #[must_use]
    pub fn misbehaving(mut self, mode: Mode, on: On) -> Self {
        self.mode = Some((mode, on));
        self
    }

    /// The backend that wants the bearer token `token`.
    #[must_use]
    pub fn wanting_token(mut self, token: &str) -> Self {
        self.token = Some(token.to_owned());
        self
    }

    /// The backend that names itself `name` at `version`.
    #[must_use]
    pub fn describing(mut self, name: &str, version: &str) -> Self {
        self.describe = (name.to_owned(), version.to_owned());
        self
    }
}

/// A running backend on 127.0.0.1. Dropping it stops it.
pub struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Starts the backend for `config` and returns once it listens.
    pub fn start(config: Config) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", config.port)).expect("a free port");
        let url = format!("http://{}", listener.local_addr().expect("an address"));
        let stop = Arc::new(AtomicBool::new(false));
        let config = Arc::new(config);
        let thread = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let config = config.clone();
                    // A connection of its own, so a hanging request holds up no other.
                    std::thread::spawn(move || serve(stream, &config));
                }
            })
        };
        Server {
            url,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL, `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // A connection unblocks the `accept` that waits for the next request.
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(mut stream: TcpStream, config: &Config) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    let goal = || {
        let body: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
        let goal = body.get("goal").and_then(Value::as_str).unwrap_or("").to_owned();
        let earlier = body.get("attempts").and_then(Value::as_array).map_or(0, Vec::len);
        (goal, earlier)
    };
    let kind = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/describe") => On::Describe,
        ("POST", "/v1/synthesize") => On::Synthesize,
        _ => {
            log(config, &format!("unknown {}", request.path));
            reply(&mut stream, 404, &[], r#"{"error":"no such endpoint"}"#);
            return;
        }
    };
    if kind == On::Synthesize {
        let (goal, _) = goal();
        log(config, &format!("synthesize {goal}"));
        if let Some(path) = &config.capture {
            std::fs::write(path, &request.body).expect("capture");
        }
    } else {
        log(config, "describe");
    }
    if let Some(path) = &config.auth_dump
        && let Some(auth) = request.headers.get("authorization")
    {
        std::fs::write(path, auth).expect("auth dump");
    }
    if let Some(token) = &config.token
        && request.headers.get("authorization").map(String::as_str) != Some(format!("Bearer {token}").as_str())
    {
        reply(&mut stream, 401, &[], r#"{"error":"unauthorized"}"#);
        return;
    }
    if let Some((mode, on)) = &config.mode
        && (*on == On::Both || *on == kind)
    {
        misbehave(&mut stream, mode);
        return;
    }
    let body = match kind {
        On::Describe => describe(config),
        _ => {
            let (goal, earlier) = goal();
            synthesize(config, &goal, earlier)
        }
    };
    reply(&mut stream, 200, &[], &body.to_string());
}

fn log(config: &Config, line: &str) {
    if let Some(path) = &config.log {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("log");
        writeln!(file, "{line}").expect("log line");
    }
}

fn describe(config: &Config) -> Value {
    let (name, version) = &config.describe;
    let mut reply = json!({"backend": name, "backend_version": version});
    if config.describe_extra {
        reply
            .as_object_mut()
            .expect("an object")
            .insert("protocol_notes".to_owned(), json!("unknown keys are ignored"));
    }
    reply
}

fn synthesize(config: &Config, goal: &str, earlier: usize) -> Value {
    let dir = config.dir.clone().expect("a reply directory");
    let numbered = dir.join(format!("{goal}.{earlier}.json"));
    let path = if earlier > 0 && numbered.is_file() {
        numbered
    } else {
        dir.join(format!("{goal}.json"))
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return json!({"error": format!("no reply for goal {goal}")});
    };
    let doc: Value = serde_json::from_str(&text).expect("a JSON reply file");
    if doc.get("kind").is_some() {
        json!({"body": doc})
    } else {
        doc
    }
}

fn misbehave(stream: &mut TcpStream, mode: &Mode) {
    match mode {
        Mode::Status(status) => reply(stream, *status, &[], r#"{"note":"a test failure"}"#),
        Mode::Garbage => reply(stream, 200, &[], "this is \u{1b}[31mnot JSON"),
        Mode::Hostile => reply(stream, 200, &[], "\u{1b}]52;c;Zm9v\u{7} evil \u{202e}txet \u{85} end"),
        Mode::Huge => reply(stream, 200, &[], &"x".repeat(3 * 1024 * 1024)),
        Mode::Redirect => reply(stream, 302, &[("location", "/v1/elsewhere")], "moved"),
        Mode::Hang => {
            // Held open until the client hangs up.
            let mut sink = [0_u8; 64];
            while stream.read(&mut sink).is_ok_and(|n| n > 0) {}
        }
    }
}

fn reply(stream: &mut TcpStream, status: u16, headers: &[(&str, &str)], body: &str) {
    let mut text = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    text.push_str(body);
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}
