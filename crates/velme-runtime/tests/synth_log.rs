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
    AnthropicConfig, Identity, ProviderError, ReplyMeta, Scripted, Step, SynthBackend, SynthLimits, SynthOptions,
    SynthProvider, SynthReply, SynthRequest, Usage, WallClock,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{PanicProvider, RecordingSleeper, body_reply, mock_anthropic, program};

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
    run_source(dir, backend, mode, &source())
}

fn run_source(dir: &Path, backend: &dyn SynthBackend, mode: Mode, text: &str) -> BuildReport {
    run_with(dir, backend, mode, text, SynthOptions::default())
}

fn run_with(dir: &Path, backend: &dyn SynthBackend, mode: Mode, text: &str, options: SynthOptions) -> BuildReport {
    let program = program(text);
    let input = BuildInput {
        mode,
        program: &program,
        source: text,
        project: dir,
        file: FILE,
        backend: Some(backend),
        options,
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
        "{{\"format\":\"velme-synth-log/2\",\"time\":\"2026-09-30T12:00:00.000Z\",\"file\":\"game.velme\",\"goal\":\"Double\",\"key\":\"{key}\",\"provider\":\"scripted\",\"model\":\"scripted\",\"attempt\":0,\"outcome\":\"VL0401\",\"cause\":\"not-json\",\"stop_reason\":null,\"reply_shape\":{{\"blocks\":null,\"keys\":[]}},\"tokens_in\":null,\"tokens_out\":null,\"cache_read_tokens\":null,\"cache_write_tokens\":null,\"latency_ms\":"
    );
    assert!(
        lines[0].ends_with(",\"reply_bytes\":8,\"reply_format\":\"ir-json\"}"),
        "{}",
        lines[0]
    );
    assert!(lines[0].starts_with(&first), "{}", lines[0]);
    let second = format!(
        "{{\"format\":\"velme-synth-log/2\",\"time\":\"2026-09-30T12:00:00.250Z\",\"file\":\"game.velme\",\"goal\":\"Double\",\"key\":\"{key}\",\"provider\":\"scripted\",\"model\":\"scripted\",\"attempt\":1,\"outcome\":\"ok\",\"cause\":null,\"stop_reason\":null,\"reply_shape\":{{\"blocks\":null,\"keys\":[\"body\"]}},\"tokens_in\":null,\"tokens_out\":null,\"cache_read_tokens\":null,\"cache_write_tokens\":null,\"latency_ms\":0,\"reply_bytes\":{},\"reply_format\":\"ir-json\"}}",
        body_reply(&double()).len()
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
            meta: ReplyMeta::default(),
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
        text.contains("\"tokens_in\":5,\"tokens_out\":null,\"cache_read_tokens\":null,\"cache_write_tokens\":null,\"latency_ms\":7,"),
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
        text.contains("\"attempt\":0,\"outcome\":\"VL0404\",\"cause\":\"unavailable\",\"stop_reason\":null,\"reply_shape\":null,\"tokens_in\":null,\"tokens_out\":null,\"cache_read_tokens\":null"),
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

/// The lines of the log as JSON.
fn lines(dir: &Path) -> Vec<Value> {
    log(dir)
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

/// A Messages API response with `content`, the `stop_reason` and a `usage` of `cache` counts.
fn message(stop_reason: &str, content: &Value, usage: &Value) -> MockResponse {
    MockResponse::ok(json!({"stop_reason": stop_reason, "content": content, "usage": usage}).to_string())
}

/// A build against a mocked `anthropic` writes the v2 members: `cause` null on `ok` and a validator rule id on a
/// failure, the provider's `stop_reason`, the reply's shape with a block kind and a reply key outside the sets as `"…"`,
/// and the cache counts as reported, `0` included; nothing the provider wrote reaches the log (AC-SYNTH-49, D-154).
#[test]
fn ac_synth_49_an_anthropic_build_logs_cause_stop_reason_shape_and_cache_counts() {
    let dir = project("v2_anthropic");
    let body = json!({"kind": "input", "name": "BODY-SENTINEL"});
    let first = message(
        "tool_use",
        &json!([
            {"type": "text", "text": "TEXT-SENTINEL"},
            {"type": "tool_use", "id": "t1", "name": "write_goal", "input": {"body": body, "KEY-SENTINEL": 1}},
            {"type": "KIND-SENTINEL", "x": 1},
        ]),
        &json!({"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 7}),
    );
    let reply: Value = serde_json::from_str(&body_reply(&double())).expect("a reply");
    let server = MockServer::start([first, MockResponse::tool_call("write_goal", &reply)]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    let report = run(&dir, &backend, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Synthesized));
    let text = log(&dir);
    for sentinel in [
        "BODY-SENTINEL",
        "TEXT-SENTINEL",
        "KEY-SENTINEL",
        "KIND-SENTINEL",
        SECRET,
    ] {
        assert!(!text.contains(sentinel), "{sentinel} in the log: {text}");
    }
    let lines = lines(&dir);
    assert_eq!(lines.len(), 2, "{text}");
    let (bad, good) = (&lines[0], &lines[1]);
    assert_eq!(bad["format"], "velme-synth-log/2");
    assert_eq!(bad["outcome"], "VL0402");
    // The body names an input the goal doesn't have.
    assert_eq!(bad["cause"], "names-7");
    assert_eq!(bad["stop_reason"], "tool_use");
    assert_eq!(
        bad["reply_shape"],
        json!({"blocks": ["text", "tool_use", "…"], "keys": ["body", "…"]})
    );
    assert_eq!(
        (&bad["cache_read_tokens"], &bad["cache_write_tokens"]),
        (&json!(0), &json!(7))
    );
    assert_eq!(bad["tokens_in"], 10);
    assert_eq!(good["outcome"], "ok");
    assert_eq!(good["cause"], Value::Null);
    assert_eq!(good["reply_shape"], json!({"blocks": ["tool_use"], "keys": ["body"]}));
    assert_eq!(
        (&good["cache_read_tokens"], &good["cache_write_tokens"]),
        (&json!(5), &json!(3))
    );
    // The members are in the order of R-SYNTH-23.
    let first = text.lines().next().expect("a line");
    let members = [
        "format",
        "time",
        "file",
        "goal",
        "key",
        "provider",
        "model",
        "attempt",
        "outcome",
        "cause",
        "stop_reason",
        "reply_shape",
        "tokens_in",
        "tokens_out",
        "cache_read_tokens",
        "cache_write_tokens",
        "latency_ms",
        "reply_bytes",
        "reply_format",
    ];
    let at: Vec<usize> = members
        .iter()
        .map(|m| first.find(&format!("\"{m}\":")).expect("a member"))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{first}");
}

/// A `stop_reason` outside `a`-`z` and `_` is `"…"`, and one longer than 32 characters too (AC-SYNTH-49).
#[test]
fn ac_synth_49_a_stop_reason_outside_the_set_is_written_as_an_ellipsis() {
    let dir = project("v2_stop_reason");
    let reply: Value = serde_json::from_str(&body_reply(&double())).expect("a reply");
    let content = json!([{"type": "tool_use", "id": "t", "name": "write_goal", "input": reply}]);
    let server = MockServer::start([message("STOP-SENTINEL", &content, &json!({}))]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    run(&dir, &backend, Mode::Build);
    let text = log(&dir);
    assert!(!text.contains("STOP-SENTINEL"), "{text}");
    let line = &lines(&dir)[0];
    assert_eq!(line["stop_reason"], "…");
    // A usage without cache fields is not a report of cache use.
    assert_eq!(
        (&line["cache_read_tokens"], &line["cache_write_tokens"]),
        (&Value::Null, &Value::Null)
    );
}

/// A check or an example that fails is `check-<n>` or `example-<n>`, 1-based in source order (AC-SYNTH-49, R-SYNTH-23).
#[test]
fn ac_synth_49_a_failed_check_or_example_names_its_place() {
    let dir = project("v2_check");
    let checked = source().replace(
        "    examples:",
        "    check:\n        - result >= 0\n        - result == n * 3\n    examples:",
    );
    run_source(&dir, &Scripted::replies([body_reply(&double())]), Mode::Build, &checked);
    let line = &lines(&dir)[0];
    assert_eq!(
        (&line["outcome"], &line["cause"]),
        (&json!("VL0501"), &json!("check-2")),
        "{line}"
    );
    let dir = project("v2_example");
    let wrong = body_reply(&double()).replace("\"mul\"", "\"sub\"");
    run(&dir, &Scripted::replies([wrong]), Mode::Build);
    let line = &lines(&dir)[0];
    assert_eq!(line["cause"], "example-1", "{line}");
}

/// Every `Malformed` attempt is one cause, and the log tells the reasons apart: `malformed-<reason>` (AC-SYNTH-48,
/// AC-SYNTH-49); the responses were read but held no reply, so they keep their blocks with no keys, and a sentinel in
/// the text block is nowhere in the log.
#[test]
fn ac_synth_48_the_log_cause_of_a_malformed_reply_carries_its_reason() {
    let dir = project("v2_malformed");
    let text_only = message(
        "end_turn",
        &json!([{"type": "text", "text": "TEXT-SENTINEL"}]),
        &json!({}),
    );
    let unknown_tool = message(
        "tool_use",
        &json!([{"type": "tool_use", "id": "t", "name": "other", "input": {}}]),
        &json!({}),
    );
    let server = MockServer::start([text_only, unknown_tool]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    let report = run(&dir, &backend, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Failed);
    assert!(!log(&dir).contains("TEXT-SENTINEL"));
    let lines = lines(&dir);
    let causes: Vec<&Value> = lines.iter().map(|l| &l["cause"]).collect();
    assert_eq!(
        causes,
        [&json!("malformed-not-one-tool"), &json!("malformed-unknown-tool")]
    );
    // The response parsed, so its stop reason and block kinds are there, and no reply keys.
    let shapes: Vec<(&Value, &Value)> = lines.iter().map(|l| (&l["stop_reason"], &l["reply_shape"])).collect();
    assert_eq!(
        shapes,
        [
            (&json!("end_turn"), &json!({"blocks": ["text"], "keys": []})),
            (&json!("tool_use"), &json!({"blocks": ["tool_use"], "keys": []})),
        ]
    );
    assert!(lines.iter().all(|l| l["outcome"] == "VL0401"));
}

/// A reply that is not a body is `no-body` with its keys in the shape, and a question is `question` (AC-SYNTH-49).
#[test]
fn ac_synth_49_replies_that_are_not_bodies_log_their_cause() {
    let dir = project("v2_causes");
    let script = Scripted::replies([
        json!({"other": 1, "error": 2}).to_string(),
        json!({"question": "Which way?"}).to_string(),
    ]);
    run(&dir, &script, Mode::Build);
    let lines = lines(&dir);
    assert_eq!(
        (&lines[0]["cause"], &lines[0]["reply_shape"]),
        (&json!("no-body"), &json!({"blocks": null, "keys": ["error", "…"]}))
    );
    assert_eq!(
        (&lines[1]["cause"], &lines[1]["outcome"]),
        (&json!("question"), &json!("VL0407"))
    );
}

/// The cause of the one line a build writes for a single scripted step.
fn scripted_cause(name: &str, step: Step, options: SynthOptions) -> Value {
    let dir = project(name);
    run_with(&dir, &Scripted::new([step]), Mode::Build, &source(), options);
    lines(&dir)[0].clone()
}

/// Each way a reply fails to be a body or a question has its own log cause (AC-SYNTH-49, R-SYNTH-23).
#[test]
fn ac_synth_49_each_reply_problem_logs_its_own_cause() {
    let reply = |text: &str| Step::Reply(text.to_owned());
    let compact = SynthOptions {
        reply_format: velme_synth::ReplyFormat::Compact,
        ..SynthOptions::default()
    };
    let body_alias = velme_synth::alias_table().keys["body"].clone();
    let cases = [
        ("not_json", reply("not json"), "not-json", SynthOptions::default()),
        (
            "dup_key",
            reply(r#"{"body":1,"body":2}"#),
            "schema-1",
            SynthOptions::default(),
        ),
        ("no_body", reply(r#"{"x":1}"#), "no-body", SynthOptions::default()),
        (
            "both",
            reply(r#"{"body":{"kind":"input","name":"n"},"question":"Which?"}"#),
            "body-and-question",
            SynthOptions::default(),
        ),
        (
            "bad_q",
            reply(r#"{"question":""}"#),
            "bad-question",
            SynthOptions::default(),
        ),
        (
            "bad_number",
            reply(r#"{"body":{"kind":"literal","value":1e999}}"#),
            "bad-number",
            SynthOptions::default(),
        ),
        (
            "alias",
            reply(&json!({ body_alias.clone(): {"zz-not-an-alias": 1} }).to_string()),
            "unknown-alias",
            compact,
        ),
        (
            "refused",
            Step::Error(ProviderError::Refused(String::new())),
            "refused",
            SynthOptions::default(),
        ),
        (
            "timeout",
            Step::Error(ProviderError::Timeout),
            "timeout",
            SynthOptions::default(),
        ),
    ];
    for (name, step, cause, options) in cases {
        let line = scripted_cause(&format!("v2_cause_{name}"), step, options);
        assert_eq!(line["cause"], cause, "{name}: {line}");
    }
}

/// A reply nested deeper than serde_json's own limit but within Velme's still logs its keys (AC-SYNTH-49).
#[test]
fn ac_synth_49_a_deep_reply_still_logs_its_keys() {
    let depth = 300;
    let deep = format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
    let reply = format!("{{\"body\":{deep}}}");
    let line = scripted_cause("v2_deep", Step::Reply(reply), SynthOptions::default());
    assert_eq!(line["reply_shape"], json!({"blocks": null, "keys": ["body"]}), "{line}");
}

/// A refusal is a failed attempt whose line still has the provider's stop reason and the block kinds, with no keys
/// (AC-SYNTH-49, D-154).
#[test]
fn ac_synth_49_an_anthropic_refusal_logs_its_stop_reason_and_blocks() {
    let dir = project("v2_refusal");
    let refusal = message(
        "refusal",
        &json!([{"type": "text", "text": "REFUSAL-SENTINEL"}]),
        &json!({}),
    );
    let server = MockServer::start([refusal]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    run_with(
        &dir,
        &backend,
        Mode::Build,
        &source(),
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
    );
    assert!(!log(&dir).contains("REFUSAL-SENTINEL"));
    let line = &lines(&dir)[0];
    assert_eq!(
        (&line["outcome"], &line["cause"]),
        (&json!("VL0401"), &json!("refused")),
        "{line}"
    );
    assert_eq!(line["stop_reason"], "refusal");
    assert_eq!(line["reply_shape"], json!({"blocks": ["text"], "keys": []}));
}

/// `ollama` logs its `done_reason`, no blocks and no cache counts, also for a reply cut off (AC-SYNTH-49, R-SYNTH-23).
#[test]
fn ac_synth_49_an_ollama_build_logs_done_reason_and_null_cache_counts() {
    let dir = project("v2_ollama");
    let cut = MockResponse::ok(
        json!({"message": {"role": "assistant", "content": "{}"}, "done": true, "done_reason": "length"}).to_string(),
    );
    let reply: Value = serde_json::from_str(&body_reply(&double())).expect("a reply");
    let server = MockServer::start([
        MockResponse::ollama_tags(&[("m:latest", "sha256:abc")]),
        cut,
        MockResponse::ollama_chat(&reply),
    ]);
    let mut config = velme_synth::OllamaConfig::new("m");
    config.url = Some(velme_synth::ExternalUrl::parse(server.url()).expect("a URL"));
    let backend = velme_synth::Ollama::new(config).with_sleeper(Arc::new(RecordingSleeper::default()));
    let report = run(&dir, &backend, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Synthesized));
    let lines = lines(&dir);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["cause"], "malformed-cut-off");
    assert_eq!(lines[0]["stop_reason"], "length");
    assert_eq!(lines[0]["reply_shape"], json!({"blocks": null, "keys": []}));
    assert_eq!(lines[1]["cause"], Value::Null);
    assert_eq!(lines[1]["stop_reason"], "stop");
    assert_eq!(lines[1]["reply_shape"], json!({"blocks": null, "keys": ["body"]}));
    for line in &lines {
        assert_eq!(
            (&line["cache_read_tokens"], &line["cache_write_tokens"]),
            (&Value::Null, &Value::Null)
        );
    }
}

/// A response that was read but held no reply (here a cut-off) still logs the tokens it billed, and the build summary
/// counts them (AC-SYNTH-49, R-SYNTH-21, D-155).
#[test]
fn ac_synth_49_a_cut_off_reply_logs_and_counts_its_tokens() {
    let dir = project("v2_cutoff_tokens");
    let cut = message(
        "max_tokens",
        &json!([{"type": "text", "text": "CUT-SENTINEL"}]),
        &json!({"input_tokens": 40, "output_tokens": 8192}),
    );
    let reply: Value = serde_json::from_str(&body_reply(&double())).expect("a reply");
    let server = MockServer::start([cut, MockResponse::tool_call("write_goal", &reply)]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    let report = run(&dir, &backend, Mode::Build);
    assert_eq!(report.goals[0].status, Status::Built(Source::Synthesized));
    let lines = lines(&dir);
    assert_eq!(lines[0]["cause"], "malformed-cut-off");
    assert_eq!(
        (&lines[0]["tokens_in"], &lines[0]["tokens_out"]),
        (&json!(40), &json!(8192))
    );
    assert_eq!(lines[0]["reply_bytes"], Value::Null);
    assert!(report.summary.usage.output_tokens >= 8192, "{:?}", report.summary.usage);
    assert!(report.summary.usage.input_tokens >= 40, "{:?}", report.summary.usage);
    assert!(!log(&dir).contains("CUT-SENTINEL"));
}

/// A `--record` build wraps the provider, and a refusal through it still logs its stop reason and blocks (AC-SYNTH-49).
#[test]
fn ac_synth_49_a_recorded_refusal_logs_its_stop_reason_and_blocks() {
    let dir = project("v2_record_refusal");
    let refusal = message("refusal", &json!([{"type": "text", "text": "R-SENTINEL"}]), &json!({}));
    let server = MockServer::start([refusal]);
    let anthropic = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    let recorder = velme_synth::Recorder::new(Box::new(anthropic), dir.join("fixtures"));
    run_with(
        &dir,
        &recorder,
        Mode::Build,
        &source(),
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
    );
    let line = &lines(&dir)[0];
    assert_eq!(line["cause"], "refused", "{line}");
    assert_eq!(line["stop_reason"], "refusal");
    assert_eq!(line["reply_shape"], json!({"blocks": ["text"], "keys": []}));
}

/// More than 64 content blocks are listed as 64 and one `"…"` (AC-SYNTH-49, D-155).
#[test]
fn ac_synth_49_the_block_list_is_capped_at_64() {
    let dir = project("v2_blocks_cap");
    let blocks: Vec<Value> = (0..70).map(|_| json!({"type": "text", "text": "x"})).collect();
    let server = MockServer::start([message("end_turn", &Value::Array(blocks), &json!({}))]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    run_with(
        &dir,
        &backend,
        Mode::Build,
        &source(),
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
    );
    let blocks = lines(&dir)[0]["reply_shape"]["blocks"].clone();
    let blocks = blocks.as_array().expect("blocks");
    assert_eq!(blocks.len(), 65);
    assert_eq!(blocks[63], "text");
    assert_eq!(blocks[64], "…");
}

/// `reply_bytes` is the reply's length as received, `null` without one, and `reply_format` is the config name the
/// request used (AC-SYNTH-49, D-155).
#[test]
fn ac_synth_49_reply_bytes_and_reply_format() {
    let dir = project("v2_bytes");
    let compact = SynthOptions {
        reply_format: velme_synth::ReplyFormat::Compact,
        ..SynthOptions::default()
    };
    let script = Scripted::new([
        Step::Error(ProviderError::Refused(String::new())),
        Step::Reply("{\"x\":1}".to_owned()),
    ]);
    run_with(&dir, &script, Mode::Build, &source(), compact);
    let lines = lines(&dir);
    assert_eq!(
        (&lines[0]["reply_bytes"], &lines[0]["reply_format"]),
        (&Value::Null, &json!("compact"))
    );
    assert_eq!(
        (&lines[1]["reply_bytes"], &lines[1]["reply_format"]),
        (&json!(7), &json!("compact"))
    );
}

/// An unknown tool name, an external backend's pending text, a backend's failure body and a rejection's message are
/// all nowhere in the log (AC-SYNTH-49, R-SYNTH-23).
#[test]
fn ac_synth_49_untrusted_text_never_reaches_the_log() {
    let dir = project("v2_secrecy_tool");
    let unknown = message(
        "tool_use",
        &json!([{"type": "tool_use", "id": "t", "name": "TOOL-SENTINEL", "input": {}}]),
        &json!({}),
    );
    let server = MockServer::start([unknown]);
    let backend = mock_anthropic(
        AnthropicConfig::new("m"),
        &server,
        "sk-ant-key",
        &RecordingSleeper::default(),
    );
    let once = SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    run_with(&dir, &backend, Mode::Build, &source(), once.clone());
    assert_eq!(lines(&dir)[0]["cause"], "malformed-unknown-tool");
    assert!(!log(&dir).contains("TOOL-SENTINEL"));
    let errors = [
        ProviderError::Pending("PENDING-SENTINEL".to_owned()),
        ProviderError::BackendFailed {
            reason: "REASON-SENTINEL".to_owned(),
            body: "BODY-SENTINEL".to_owned(),
        },
        ProviderError::Rejected {
            status: Some(400),
            message: "MESSAGE-SENTINEL".to_owned(),
        },
    ];
    for (n, error) in errors.into_iter().enumerate() {
        let dir = project(&format!("v2_secrecy_{n}"));
        run_with(
            &dir,
            &Scripted::new([Step::Error(error)]),
            Mode::Build,
            &source(),
            once.clone(),
        );
        let text = log(&dir);
        assert!(text.contains("\"cause\":\""), "{text}");
        assert!(!text.contains("SENTINEL"), "{text}");
    }
}

/// A bug of Velme's logs the cause `internal` (AC-SYNTH-49, R-SYNTH-23).
#[test]
fn ac_synth_49_an_internal_error_logs_the_internal_cause() {
    let dir = project("v2_internal");
    let script = Scripted::new([Step::Error(ProviderError::Internal("INTERNAL-SENTINEL".to_owned()))]);
    run(&dir, &script, Mode::Build);
    let line = &lines(&dir)[0];
    assert_eq!(
        (&line["outcome"], &line["cause"]),
        (&json!("VL0607"), &json!("internal")),
        "{line}"
    );
    assert!(!log(&dir).contains("SENTINEL"));
}
