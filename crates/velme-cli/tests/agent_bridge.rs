//! `tools/agent-bridge` (P0c, dev tooling): a build through the real `external` provider against the bridge. The first run
//! queues the request (`VL0408`, R-SYNTH-41) and writes it to `pending/`; an answer written under the same id is accepted
//! by the next run. The bridge is a standard-library Python script, started on a free port of 127.0.0.1.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::json;
use velme_test_support::{repo, velme_command};

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    check:
        - result == n * 2
    examples:
        - Double(2) == 4
";

/// The bridge process, killed when the test is done.
struct Bridge {
    child: Child,
    url: String,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn python() -> &'static str {
    ["python3", "python"]
        .into_iter()
        .find(|p| {
            Command::new(p)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .expect("python3 on the PATH (tools/agent-bridge is a Python script)")
}

fn start(dir: &Path, token: Option<&str>) -> Bridge {
    let mut command = Command::new(python());
    command
        .arg(repo("tools/agent-bridge/bridge.py"))
        .arg("--answers")
        .arg(dir.join("answers"))
        .arg("--pending")
        .arg(dir.join("pending"))
        .args(["--port", "0"])
        .env_remove("VELME_EXTERNAL_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(token) = token {
        command.args(["--token", token]);
    }
    let mut child = command.spawn().expect("the bridge starts");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("stdout"))
        .read_line(&mut line)
        .expect("the listening line");
    let url = line
        .trim()
        .strip_prefix("agent-bridge listening on ")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .to_owned();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    Bridge { child, url }
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("agent-bridge").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    fs::write(dir.join("game.velme"), SOURCE).expect("source");
    dir
}

/// `velme build` with the external provider at `url`: the exit code and everything it printed.
fn build(dir: &Path, url: &str, token: Option<&str>) -> (i32, String) {
    let mut command = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"));
    command
        .args(["build", "game.velme", "--provider", "external", "--external-url", url])
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_EXTERNAL_URL")
        .env_remove("VELME_EXTERNAL_TOKEN");
    if let Some(token) = token {
        command.env("VELME_EXTERNAL_TOKEN", token);
    }
    let out = command.output().expect("velme runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().expect("an exit code"), text)
}

fn only_file(dir: &Path) -> PathBuf {
    let files: Vec<_> = fs::read_dir(dir)
        .expect("directory")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.into_iter().next().expect("one file")
}

/// The first build queues the request and says which id to answer; an answer under that id is accepted by the second.
#[test]
fn p0c_the_bridge_queues_a_request_and_accepts_the_answer() {
    let dir = project("round-trip");
    let bridge = start(&dir, None);

    let (code, text) = build(&dir, &bridge.url, None);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("VL0408"), "{text}");
    let pending = only_file(&dir.join("pending"));
    let id = pending.file_stem().and_then(|s| s.to_str()).expect("an id").to_owned();
    assert!(text.contains(&id), "the note names the id: {text}");
    let request: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pending).expect("request")).expect("JSON");
    assert_eq!(request["goal"], "Double");

    // The same request gets the same id, so nothing new is queued.
    let (_, again) = build(&dir, &bridge.url, None);
    assert!(again.contains(&id), "{again}");
    assert_eq!(fs::read_dir(dir.join("pending")).expect("pending").count(), 1);

    let body = json!({"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                      "right": {"kind": "literal", "type": {"t": "Number"}, "value": 2}});
    fs::write(dir.join("answers").join(format!("{id}.json")), body.to_string()).expect("answer");
    let (code, text) = build(&dir, &bridge.url, None);
    assert_eq!(code, 0, "{text}");
}

/// A question file is passed on as it is, a half-written one is still pending, and one that is not an object is the backend failing.
#[test]
fn p0c_the_bridge_passes_on_a_question_and_refuses_a_broken_answer() {
    let dir = project("answers");
    let bridge = start(&dir, None);
    let (_, _) = build(&dir, &bridge.url, None);
    let id = only_file(&dir.join("pending"))
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("an id")
        .to_owned();
    let answer = dir.join("answers").join(format!("{id}.json"));

    fs::write(&answer, r#"{"question": "Double of what?"}"#).expect("answer");
    let (_, text) = build(&dir, &bridge.url, None);
    assert!(text.contains("Double of what?"), "{text}");

    fs::write(&answer, "not json").expect("answer");
    let (code, text) = build(&dir, &bridge.url, None);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("VL0408") && text.contains("not valid JSON yet"), "{text}");

    fs::write(&answer, "[1]").expect("answer");
    let (code, text) = build(&dir, &bridge.url, None);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("VL0406") && text.contains("not a JSON object"), "{text}");
}

/// With a token the bridge refuses a request without it (`VL0405`) and serves one with it.
#[test]
fn p0c_the_bridge_honours_the_bearer_token() {
    let token = "agent-bridge-token-0123";
    let dir = project("token");
    let bridge = start(&dir, Some(token));

    let (code, text) = build(&dir, &bridge.url, None);
    assert_ne!(code, 0);
    assert!(text.contains("VL0405"), "{text}");
    assert_eq!(fs::read_dir(dir.join("pending")).expect("pending").count(), 0);

    let (_, text) = build(&dir, &bridge.url, Some(token));
    assert!(text.contains("VL0408"), "{text}");
    assert!(!text.contains(token), "the token is never shown");
    assert_eq!(fs::read_dir(dir.join("pending")).expect("pending").count(), 1);
}

/// A compact request gets the answer as written (not wrapped), a half-written answer is still pending, and a bad
/// `Content-Length` is a 400 (P0c).
#[test]
fn p0c_the_bridge_follows_the_request_format_and_survives_a_half_written_answer() {
    use std::io::{Read, Write};
    let dir = project("compact");
    fs::write(dir.join("velme.toml"), "[synthesis]\nreply_format = \"compact\"\n").expect("velme.toml");
    let bridge = start(&dir, None);
    let (_, _) = build(&dir, &bridge.url, None);
    let pending = only_file(&dir.join("pending"));
    let id = pending.file_stem().and_then(|s| s.to_str()).expect("an id").to_owned();
    let request: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pending).expect("request")).expect("JSON");
    assert_eq!(request["reply_format"], "compact");

    let answer = dir.join("answers").join(format!("{id}.json"));
    fs::write(&answer, r#"{"bo": {"#).expect("half an answer");
    let (_, text) = build(&dir, &bridge.url, None);
    assert!(text.contains("VL0408") && text.contains("not valid JSON yet"), "{text}");

    let body = json!({"body": {"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                      "right": {"kind": "literal", "type": {"t": "Number"}, "value": 2}}});
    fs::write(&answer, velme_synth::compress(&body).to_string()).expect("answer");
    let (code, text) = build(&dir, &bridge.url, None);
    assert_eq!(code, 0, "{text}");

    let mut stream = std::net::TcpStream::connect(bridge.url.trim_start_matches("http://")).expect("connects");
    stream
        .write_all(b"POST /v1/synthesize HTTP/1.1\r\nHost: x\r\nContent-Length: -5\r\n\r\n")
        .expect("writes");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("reads");
    assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");
}
