//! The synth log (`compiler/22` R-SYNTH-23, D-109): one line per provider request in `.velme/synth-log.jsonl`, none for a
//! lock or store hit, no plan text, never through a link, and `.velme/.gitignore` beside it.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_ir::IR_VERSION;
use velme_runtime::{BuildInput, BuildReport, Mode, Options, Source, Status, build};
use velme_synth::{
    Identity, ProviderError, Scripted, Step, SynthBackend, SynthLimits, SynthOptions, SynthProvider, SynthReply,
    SynthRequest, Usage, WallClock,
};
use velme_test_support::{PanicProvider, body_reply, program};

const FILE: &str = "game.velme";
const SECRET: &str = "zebra-plan-marker-7781";

/// 2026-09-30T12:00:00.000Z, and 250 ms more at each look.
#[derive(Debug)]
struct Ticking(std::sync::atomic::AtomicI64);

impl WallClock for Ticking {
    fn unix_millis(&self) -> i64 {
        self.0.fetch_add(250, std::sync::atomic::Ordering::SeqCst)
    }
}

fn source() -> String {
    format!(
        "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it. {SECRET}\"
    examples:
        - Double(2) == 4
"
    )
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("synth_log").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

fn double() -> String {
    let n = json!({"t": "Number"});
    let body = json!({"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
        "right": {"kind": "literal", "type": n, "value": 2}});
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "Double", "types": {},
           "inputs": [["n", n]], "output": n, "body": body})
    .to_string()
}

fn run(dir: &Path, backend: &dyn SynthBackend, mode: Mode) -> BuildReport {
    let text = source();
    let program = program(&text);
    let input = BuildInput {
        mode,
        program: &program,
        source: &text,
        project: dir,
        file: FILE,
        backend: Some(backend),
        options: SynthOptions::default(),
        run: Options {
            wall_clock: Arc::new(Ticking(std::sync::atomic::AtomicI64::new(1_790_769_600_000))),
            ..Options::default()
        },
    };
    build(&input, &mut || {})
}

fn log(dir: &Path) -> String {
    fs::read_to_string(dir.join(".velme/synth-log.jsonl")).unwrap_or_default()
}

/// A build with a scripted provider: one line per `complete()` call with the R-SYNTH-23 keys in order and the injected
/// time, a failed attempt logging its code and attempt number, no plan text, a second build (a lock hit) adding nothing,
/// and `.velme/.gitignore` created (AC-SYNTH-44).
#[test]
fn ac_synth_44_one_line_per_provider_call() {
    let dir = project("lines");
    let script = Scripted::new([Step::Reply("not json".to_owned()), Step::Reply(body_reply(&double()))]);
    let report = run(&dir, &script, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Synthesized));
    let text = log(&dir);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    let key = lines[0]
        .split("\"key\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("key");
    assert!(key.starts_with("b3:"), "{key}");
    let first = format!(
        "{{\"format\":\"velme-synth-log/1\",\"time\":\"2026-09-30T12:00:00.000Z\",\"file\":\"game.velme\",\"goal\":\"Double\",\"key\":\"{key}\",\"provider\":\"scripted\",\"model\":\"scripted\",\"attempt\":0,\"outcome\":\"VL0401\",\"tokens_in\":null,\"tokens_out\":null,\"latency_ms\":"
    );
    assert!(lines[0].starts_with(&first), "{}", lines[0]);
    let second = format!(
        "{{\"format\":\"velme-synth-log/1\",\"time\":\"2026-09-30T12:00:00.250Z\",\"file\":\"game.velme\",\"goal\":\"Double\",\"key\":\"{key}\",\"provider\":\"scripted\",\"model\":\"scripted\",\"attempt\":1,\"outcome\":\"ok\",\"tokens_in\":null,\"tokens_out\":null,\"latency_ms\":0}}"
    );
    assert_eq!(lines[1], second);
    for line in &lines {
        assert!(serde_json::from_str::<Value>(line).is_ok(), "{line}");
    }
    assert!(!text.contains(SECRET), "plan text in the log: {text}");
    assert_eq!(
        fs::read_to_string(dir.join(".velme/.gitignore")).expect("gitignore"),
        "synth-log.jsonl\n"
    );
    // A lock hit reaches no provider and adds no line (AC-SYNTH-44).
    let again = run(&dir, &PanicProvider, Mode::Build);
    assert_eq!(again.goals[0].status, Status::Built(Source::Lock));
    assert_eq!(log(&dir), text);
}

/// `--locked` and `--offline` make no provider call, so they write no line (AC-SYNTH-44, R-CLI-05).
#[test]
fn ac_synth_44_locked_and_offline_write_no_line() {
    let dir = project("modes");
    for mode in [Mode::Locked, Mode::Offline] {
        let _ = run(&dir, &PanicProvider, mode);
    }
    assert_eq!(log(&dir), "");
    assert!(!dir.join(".velme/synth-log.jsonl").exists());
}

/// A `.velme/.gitignore` that is already there is kept as it is (AC-SYNTH-44).
#[test]
fn ac_synth_44_an_existing_gitignore_is_kept() {
    let dir = project("gitignore");
    fs::create_dir_all(dir.join(".velme")).expect("dir");
    fs::write(dir.join(".velme/.gitignore"), "mine\n").expect("write");
    let script = Scripted::replies([body_reply(&double())]);
    run(&dir, &script, Mode::Build);
    assert_eq!(
        fs::read_to_string(dir.join(".velme/.gitignore")).expect("read"),
        "mine\n"
    );
    assert_eq!(log(&dir).lines().count(), 1);
}

/// A link at the log path is not followed: the build still succeeds and `-v` gets a notice (AC-SYNTH-44).
#[cfg(unix)]
#[test]
fn ac_synth_44_a_link_at_the_log_path_is_refused() {
    let dir = project("link");
    fs::create_dir_all(dir.join(".velme")).expect("dir");
    let target = dir.join("elsewhere.txt");
    fs::write(&target, "keep\n").expect("write");
    std::os::unix::fs::symlink(&target, dir.join(".velme/synth-log.jsonl")).expect("link");
    let script = Scripted::replies([body_reply(&double())]);
    let report = run(&dir, &script, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Synthesized));
    assert_eq!(fs::read_to_string(&target).expect("read"), "keep\n");
    assert!(
        report.goals[0].attempts.iter().any(|a| a.contains("synth log")),
        "{:?}",
        report.goals[0].attempts
    );
}

/// A provider that reports 5 tokens sent and none received, and took 7 ms.
struct Counting;

#[async_trait]
impl SynthBackend for Counting {
    async fn identify(&self) -> Result<Identity, ProviderError> {
        Ok(Identity {
            provider: "counting".to_owned(),
            model: "m".to_owned(),
            input_version: "1".to_owned(),
            backend: None,
        })
    }

    fn open(&self, _: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(Counting))
    }
}

#[async_trait]
impl SynthProvider for Counting {
    fn id(&self) -> &str {
        "counting"
    }

    fn model(&self) -> &str {
        "m"
    }

    fn input_version(&self) -> &str {
        "1"
    }

    async fn complete(&self, _: &SynthRequest, _: &SynthLimits) -> Result<SynthReply, ProviderError> {
        Ok(SynthReply {
            reply_json: body_reply(&double()),
            usage: Usage {
                input_tokens: 5,
                ..Usage::default()
            },
            latency: Duration::from_millis(7),
        })
    }
}

/// Each token count is its own number, or `null` when it is 0 or unreported (AC-SYNTH-44, R-SYNTH-23).
#[test]
fn ac_synth_44_token_counts_are_null_one_by_one() {
    let dir = project("tokens");
    run(&dir, &Counting, Mode::Build);
    let text = log(&dir);
    assert!(
        text.contains("\"tokens_in\":5,\"tokens_out\":null,\"latency_ms\":7}"),
        "{text}"
    );
}

/// A transport error is a request too: one `VL0404` line with null tokens (AC-SYNTH-44).
#[test]
fn ac_synth_44_a_provider_error_logs_its_code() {
    let dir = project("error");
    let script = Scripted::new([Step::Error(ProviderError::Unavailable("down".to_owned()))]);
    let report = run(&dir, &script, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Failed);
    let text = log(&dir);
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(
        text.contains("\"attempt\":0,\"outcome\":\"VL0404\",\"tokens_in\":null,\"tokens_out\":null"),
        "{text}"
    );
}

/// A store hit reaches no provider and adds no line (AC-SYNTH-44).
#[test]
fn ac_synth_44_a_store_hit_adds_no_line() {
    let dir = project("store_hit");
    run(&dir, &Scripted::replies([body_reply(&double())]), Mode::Build);
    let before = log(&dir);
    assert_eq!(before.lines().count(), 1);
    fs::remove_file(dir.join("velme.lock")).expect("lock");
    let report = run(&dir, &Scripted::replies(Vec::<String>::new()), Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Store));
    assert_eq!(log(&dir), before);
}

/// When the log and the store both fail, the `-v` notice is still there (AC-SYNTH-44).
#[cfg(unix)]
#[test]
fn ac_synth_44_the_notice_survives_a_store_failure() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = project("both_fail");
    let artifacts = dir.join(".velme/artifacts");
    fs::create_dir_all(&artifacts).expect("dir");
    fs::create_dir_all(dir.join(".velme/tmp")).expect("dir");
    std::os::unix::fs::symlink(dir.join("x"), dir.join(".velme/synth-log.jsonl")).expect("link");
    fs::set_permissions(&artifacts, fs::Permissions::from_mode(0o555)).expect("chmod");
    fs::set_permissions(dir.join(".velme/tmp"), fs::Permissions::from_mode(0o555)).expect("chmod");
    let report = run(&dir, &Scripted::replies([body_reply(&double())]), Mode::Build);
    fs::set_permissions(&artifacts, fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::set_permissions(dir.join(".velme/tmp"), fs::Permissions::from_mode(0o755)).expect("chmod");
    let goal = &report.goals[0];
    if goal.status == Status::Failed {
        assert!(
            goal.attempts.iter().any(|a| a.contains("synth log")),
            "{:?}",
            goal.attempts
        );
    }
}
