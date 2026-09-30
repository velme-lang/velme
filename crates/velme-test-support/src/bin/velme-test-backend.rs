//! A tiny deterministic `external` backend (`compiler/22` §3.2, D-42, D-99): it answers the protocol from hand-written IR
//! and misbehaves on request, so the tests of the `external` provider and the unattended recording of the examples'
//! replay fixtures need no live provider. Not a product: nothing here is reachable from `velme`.
//!
//! ```text
//! velme-test-backend --dir DIR [--log FILE] [--env-dump FILE]
//!                    [--capture FILE] [--describe NAME VERSION] [--describe-extra] [--sleeper --pidfile FILE] [--mode MODE [--on describe|synthesize|both] [--pidfile FILE]]
//! ```
//!
//! `synthesize` for goal `G` answers with `DIR/G.json` (`DIR/G.N.json` when the request carries `N` earlier attempts,
//! else `G.json`): a file holding IR is wrapped as `{"ir": …}`, any other file is sent as it is. A goal with no file is
//! an `{"error"}` reply. `--mode` makes the backend `exit` with status 3, `hang`, `hang-group` (a child that also
//! hangs, its pid written to `--pidfile`), print `garbage` (with a line on stderr) or a `huge` output. `--sleeper` leaves a `sleep 60` behind
//! that holds the backend's stdout open (its pid in `--pidfile`), then replies and exits as usual.
#![forbid(unsafe_code)]
// A tool for tests: it fails loudly on a bad invocation.
#![allow(clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

#[derive(Default)]
struct Args {
    dir: Option<PathBuf>,
    log: Option<PathBuf>,
    env_dump: Option<PathBuf>,
    capture: Option<PathBuf>,
    pidfile: Option<PathBuf>,
    describe: Option<(String, String)>,
    describe_extra: bool,
    sleeper: bool,
    mode: Option<String>,
    on: String,
}

fn parse() -> Args {
    let mut args = Args {
        on: "synthesize".to_owned(),
        ..Args::default()
    };
    let mut rest = std::env::args().skip(1);
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().expect("a value after the flag");
        match flag.as_str() {
            "--dir" => args.dir = Some(PathBuf::from(value())),
            "--log" => args.log = Some(PathBuf::from(value())),
            "--env-dump" => args.env_dump = Some(PathBuf::from(value())),
            "--capture" => args.capture = Some(PathBuf::from(value())),
            "--pidfile" => args.pidfile = Some(PathBuf::from(value())),
            "--describe" => {
                let name = value();
                args.describe = Some((name, value()));
            }
            "--describe-extra" => args.describe_extra = true,
            "--sleeper" => args.sleeper = true,
            "--mode" => args.mode = Some(value()),
            "--on" => args.on = value(),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn main() -> ExitCode {
    let args = parse();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).expect("stdin");
    let message: Value = serde_json::from_str(&input).expect("a JSON message");
    let kind = message.get("kind").and_then(Value::as_str).unwrap_or("");
    if let Some(path) = &args.capture {
        std::fs::write(path, &input).expect("capture");
    }
    if let Some(path) = &args.env_dump {
        let mut vars: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        vars.sort();
        std::fs::write(path, vars.join("\n")).expect("env dump");
    }
    if let Some(path) = &args.log {
        let goal = message.pointer("/request/goal").and_then(Value::as_str).unwrap_or("");
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("log");
        writeln!(log, "{}", format!("{kind} {goal}").trim()).expect("log line");
    }
    if args.sleeper {
        leave_sleeper(&args);
    }
    if let Some(mode) = &args.mode
        && (args.on == "both" || args.on == kind)
    {
        return misbehave(mode, &args);
    }
    let reply = match kind {
        "describe" => {
            let (name, version) = args
                .describe
                .clone()
                .unwrap_or_else(|| ("velme-test-support".to_owned(), "hand-written".to_owned()));
            let mut reply = json!({"backend": name, "backend_version": version});
            if args.describe_extra {
                reply
                    .as_object_mut()
                    .expect("an object")
                    .insert("protocol_notes".to_owned(), json!("unknown keys are ignored"));
            }
            reply
        }
        "synthesize" => synthesize(&args, &message),
        other => json!({"error": format!("unknown message kind {other}")}),
    };
    println!("{reply}");
    ExitCode::SUCCESS
}

fn synthesize(args: &Args, message: &Value) -> Value {
    let goal = message.pointer("/request/goal").and_then(Value::as_str).unwrap_or("");
    let earlier = message
        .pointer("/request/attempts")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let dir = args.dir.clone().expect("--dir");
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
    if doc.get("ir_version").is_some() {
        json!({"ir": doc})
    } else {
        doc
    }
}

fn misbehave(mode: &str, args: &Args) -> ExitCode {
    match mode {
        "exit" => {
            eprintln!("\u{1b}[31mbackend exploded\u{1b}[0m: it is a test");
            ExitCode::from(3)
        }
        "garbage" => {
            eprintln!("garbage mode: printing what is not JSON");
            println!("this is not JSON");
            ExitCode::SUCCESS
        }
        "huge" => {
            let chunk = "x".repeat(64 * 1024);
            let mut out = std::io::stdout();
            for _ in 0..64 {
                if out.write_all(chunk.as_bytes()).is_err() {
                    break;
                }
            }
            ExitCode::SUCCESS
        }
        "hang" => hang(),
        "hang-group" => {
            leave_sleeper(args);
            hang()
        }
        other => panic!("unknown mode {other}"),
    }
}

/// Starts a `sleep 60` that inherits stdout and stderr, and writes its pid to `--pidfile`. Left running on purpose: the
/// supervisor's group kill is what stops it.
fn leave_sleeper(args: &Args) {
    #[allow(clippy::zombie_processes)]
    let child = Command::new("sleep").arg("60").spawn().expect("a child");
    if let Some(path) = &args.pidfile {
        std::fs::write(path, child.id().to_string()).expect("pidfile");
    }
}

fn hang() -> ExitCode {
    std::thread::sleep(std::time::Duration::from_secs(60));
    ExitCode::SUCCESS
}
