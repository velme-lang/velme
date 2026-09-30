//! `build` (`compiler/20` R-CMP-20, `runtime/32` R-ART-15, `compiler/22` R-SYNTH-02): the lookup order, post-order
//! synthesis, blocked ancestors, re-verification of ancestors, and the artifacts and lock a build writes.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_diagnostics::Code;
use velme_ir::IR_VERSION;
use velme_runtime::{BuildInput, BuildReport, Clock, Lock, Options, Source, Status, build};
use velme_synth::{
    AnthropicConfig, Identity, ProviderError, Recorder, Replay, Scripted, Step, SynthBackend, SynthOptions,
    SynthProvider,
};
use velme_test_support::mock::{MockResponse, MockServer};
use velme_test_support::{PanicProvider, RecordingSleeper, install, mock_anthropic, program};

const FILE: &str = "game.velme";

fn source(double_plan: &str) -> String {
    format!(
        "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"{double_plan}\"
    examples:
        - Double(2) == 4

goal AddOne(n: Number) -> Number:
    plan: \"Add one.\"
    examples:
        - AddOne(1) == 2

goal Sum(n: Number) -> Number:
    call:
        d = Double(n)
    plan: \"Add one to d.\"
    check:
        - result == n * 2 + 1
    examples:
        - Sum(3) == 7

goal Both(n: Number) -> Number:
    call:
        d = Double(n)
        result = AddOne(d)
"
    )
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("build").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

fn number() -> Value {
    json!({"t": "Number"})
}

fn input() -> Value {
    json!({"kind": "input", "name": "n"})
}

fn literal(n: i64) -> Value {
    json!({"kind": "literal", "type": number(), "value": n})
}

fn ir(goal: &str, body: &Value) -> String {
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal, "types": {},
           "inputs": [["n", number()]], "output": number(), "body": body})
    .to_string()
}

fn double() -> String {
    ir(
        "Double",
        &json!({"kind": "binary", "op": "mul", "left": input(), "right": literal(2)}),
    )
}

/// `Double` that differs from `n * 2` at 3 only, so its example still holds.
fn double_off_at_three() -> String {
    ir(
        "Double",
        &json!({"kind": "if", "cond": {"kind": "binary", "op": "eq", "left": input(), "right": literal(3)},
                "then": literal(0), "else": {"kind": "binary", "op": "mul", "left": input(), "right": literal(2)}}),
    )
}

fn add_one() -> String {
    ir(
        "AddOne",
        &json!({"kind": "binary", "op": "add", "left": input(), "right": literal(1)}),
    )
}

fn sum() -> String {
    ir(
        "Sum",
        &json!({"kind": "binary", "op": "add", "left": {"kind": "local", "name": "d"}, "right": literal(1)}),
    )
}

/// `Sum` that does not use its child.
fn sum_alone() -> String {
    ir(
        "Sum",
        &json!({"kind": "binary", "op": "add", "left": {"kind": "binary", "op": "mul", "left": input(), "right": literal(2)},
                "right": literal(1)}),
    )
}

fn replies(list: &[String]) -> Scripted {
    Scripted::replies(list.iter().cloned())
}

struct Built {
    report: BuildReport,
    contacts: usize,
}

fn build_with(dir: &Path, text: &str, backend: Option<&dyn SynthBackend>, options: SynthOptions) -> Built {
    let program = program(text);
    let mut contacts = 0;
    let input = BuildInput {
        program: &program,
        source: text,
        project: dir,
        file: FILE,
        backend,
        options,
        run: Options::default(),
    };
    let report = build(&input, &mut || contacts += 1);
    Built { report, contacts }
}

fn status(built: &Built, goal: &str) -> Status {
    built
        .report
        .goals
        .iter()
        .find(|g| g.goal == goal)
        .unwrap_or_else(|| panic!("no goal {goal}"))
        .status
}

fn diagnostics(built: &Built, goal: &str) -> Vec<velme_diagnostics::Diagnostic> {
    built
        .report
        .goals
        .iter()
        .find(|g| g.goal == goal)
        .expect("goal")
        .diagnostics
        .clone()
}

/// Every file under the project's `.velme/artifacts` and its lock, by name, for comparing two builds.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![(
        "velme.lock".to_owned(),
        fs::read(dir.join("velme.lock")).unwrap_or_default(),
    )];
    if let Ok(entries) = fs::read_dir(dir.join(".velme/artifacts")) {
        let mut found: Vec<_> = entries.filter_map(Result::ok).collect();
        found.sort_by_key(std::fs::DirEntry::file_name);
        for e in found {
            files.push((
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).expect("artifact"),
            ));
        }
    }
    files
}

fn full_script() -> Scripted {
    replies(&[double(), add_one(), sum()])
}

/// Building twice: the second build makes zero provider calls, contacts no provider at all, and leaves the lock and the
/// store byte-identical (AC-ART-01, AC-SYNTH-01, AC-CMP-02's panicking provider, R-ART-17).
#[test]
fn ac_art_01_a_second_build_makes_no_provider_call() {
    let dir = project("twice");
    let text = source("Double it.");
    let script = full_script();
    let first = build_with(&dir, &text, Some(&script), SynthOptions::default());
    assert_eq!(status(&first, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(status(&first, "Sum"), Status::Built(Source::Synthesized));
    assert_eq!(status(&first, "Both"), Status::Built(Source::Compiler));
    assert_eq!((first.report.summary.calls, first.contacts), (3, 1));
    let before = snapshot(&dir);
    let second = build_with(&dir, &text, Some(&PanicProvider), SynthOptions::default());
    for goal in ["Double", "AddOne"] {
        assert_eq!(status(&second, goal), Status::Built(Source::Lock), "{goal}");
    }
    // A goal with calls is checked again against them, on the interpreter and with no provider (R-ART-22).
    for goal in ["Sum", "Both"] {
        assert_eq!(status(&second, goal), Status::Built(Source::Reverified), "{goal}");
    }
    assert_eq!((second.report.summary.calls, second.contacts), (0, 0));
    assert_eq!(snapshot(&dir), before);
}

/// A wired goal builds with no provider at all, and its artifact records `compiler` (AC-ART-09, AC-CMP-06).
#[test]
fn ac_art_09_a_wired_goal_builds_without_a_provider() {
    let dir = project("wired");
    let text = source("Double it.");
    let program = program(&text);
    install(&dir, FILE, &program, &double());
    install(&dir, FILE, &program, &add_one());
    let built = build_with(&dir, &text, None, SynthOptions::default());
    assert_eq!(status(&built, "Both"), Status::Built(Source::Compiler));
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let entry = lock.entry(FILE, "Both").expect("an entry");
    let artifact = velme_runtime::Store::new(&dir).get(entry.artifact).expect("artifact");
    assert_eq!(artifact.manifest.provider, "compiler");
    assert_eq!(artifact.manifest.model_version, None);
    // The goals that needed a provider have none: `Sum` fails with `VL0404`, which `Both` doesn't call.
    assert_eq!(diagnostics(&built, "Sum")[0].code, Code::ProviderUnavailable);
}

/// Changing only `Double`'s plan re-synthesizes `Double` only; `Sum`'s entry is unchanged (AC-ART-02, AC-CMP-05).
#[test]
fn ac_art_02_only_the_edited_goal_is_synthesized_again() {
    let dir = project("edit");
    let first = build_with(
        &dir,
        &source("Double it."),
        Some(&full_script()),
        SynthOptions::default(),
    );
    assert_eq!(first.report.summary.calls, 3);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let (sum, double_before) = (
        lock.entry(FILE, "Sum").expect("entry").clone(),
        lock.entry(FILE, "Double").expect("entry").clone(),
    );
    let script = replies(&[double()]);
    let second = build_with(
        &dir,
        &source("Twice the number."),
        Some(&script),
        SynthOptions::default(),
    );
    assert_eq!(second.report.summary.calls, 1);
    assert_eq!(status(&second, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(status(&second, "AddOne"), Status::Built(Source::Lock));
    // `Sum` is checked again against the new `Double`, with no call (R-ART-22).
    assert_eq!(status(&second, "Sum"), Status::Built(Source::Reverified));
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert_eq!(lock.entry(FILE, "Sum"), Some(&sum));
    assert_ne!(lock.entry(FILE, "Double"), Some(&double_before));
}

/// A model change with an up-to-date lock makes no call and no contact (AC-ART-03).
#[test]
fn ac_art_03_a_new_model_with_a_fresh_lock_makes_no_call() {
    struct Model(Scripted, &'static str);
    #[async_trait]
    impl SynthBackend for Model {
        async fn identify(&self) -> Result<Identity, ProviderError> {
            let mut identity = self.0.identify().await?;
            identity.model = self.1.to_owned();
            Ok(identity)
        }
        fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
            self.0.open(identity)
        }
    }
    let dir = project("model");
    let text = source("Double it.");
    let first = build_with(
        &dir,
        &text,
        Some(&Model(full_script(), "model-a")),
        SynthOptions::default(),
    );
    assert_eq!(first.report.summary.calls, 3);
    let second = build_with(&dir, &text, Some(&PanicProvider), SynthOptions::default());
    assert_eq!((second.report.summary.calls, second.contacts), (0, 0));
    // Without the lock, the store is asked under the new model's key and misses; under the old one it hits (D-92).
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let hit = build_with(
        &dir,
        &text,
        Some(&Model(replies(&[]), "model-a")),
        SynthOptions::default(),
    );
    assert_eq!((hit.report.summary.calls, hit.report.summary.store_hits), (0, 3));
    assert_eq!(status(&hit, "Sum"), Status::Built(Source::Store));
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let miss = build_with(
        &dir,
        &text,
        Some(&Model(full_script(), "model-b")),
        SynthOptions::default(),
    );
    assert_eq!(miss.report.summary.calls, 3);
}

/// A changed child that leaves the parent passing costs no call; one that breaks it re-synthesizes the parent, its first
/// request carrying the old IR and the failure, and the learner sees a note naming the child (AC-ART-10, R-SYNTH-46).
#[test]
fn ac_art_10_a_parent_is_checked_against_a_changed_child() {
    let dir = project("reverify");
    let first = build_with(
        &dir,
        &source("Double it."),
        Some(&full_script()),
        SynthOptions::default(),
    );
    assert_eq!(first.report.summary.calls, 3);
    let script = replies(&[double_off_at_three(), sum_alone()]);
    let second = build_with(&dir, &source("Twice."), Some(&script), SynthOptions::default());
    assert_eq!(status(&second, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(status(&second, "Sum"), Status::Built(Source::Synthesized));
    assert_eq!(second.report.summary.calls, 2);
    let requests = script.requests();
    let attempt = &requests[1].attempts[0];
    assert!(attempt.reply.contains("\"goal\":\"Sum\""), "{}", attempt.reply);
    assert_eq!(attempt.diagnostics[0].code, "VL0503");
    let sum = second.report.goals.iter().find(|g| g.goal == "Sum").expect("goal");
    assert!(
        sum.notes.iter().any(|n| n.contains("`Double` changed")),
        "{:?}",
        sum.notes
    );
}

/// A question ends its goal at once, storing nothing and leaving the lock without it, while another goal in the same file
/// still builds (AC-SYNTH-22).
#[test]
fn ac_synth_22_a_question_leaves_other_goals_building() {
    let dir = project("question");
    let script = Scripted::new([
        Step::Reply(r#"{"question": "Round up?"}"#.to_owned()),
        Step::Reply(add_one()),
    ]);
    let built = build_with(&dir, &source("Double it."), Some(&script), SynthOptions::default());
    assert_eq!(status(&built, "Double"), Status::Failed);
    assert_eq!(diagnostics(&built, "Double")[0].code, Code::PlanUnclear);
    assert_eq!(status(&built, "AddOne"), Status::Built(Source::Synthesized));
    assert_eq!(script.calls(), 3 - 1);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert!(lock.entry(FILE, "Double").is_none());
    assert!(lock.entry(FILE, "AddOne").is_some());
    assert_eq!(snapshot(&dir).len(), 2, "one artifact and the lock");
}

/// A goal with no artifact blocks its ancestors, which get `VL0409` naming the child and its code, with no call for them
/// (AC-SYNTH-33, R-SYNTH-42).
#[test]
fn ac_synth_33_a_failed_child_blocks_its_ancestors() {
    let dir = project("blocked");
    let script = Scripted::new([Step::Reply("not json".to_owned()), Step::Reply(add_one())]);
    let options = SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    let built = build_with(&dir, &source("Double it."), Some(&script), options);
    assert_eq!(diagnostics(&built, "Double")[0].code, Code::SynthesisFailed);
    for goal in ["Sum", "Both"] {
        assert_eq!(status(&built, goal), Status::Blocked, "{goal}");
        let d = &diagnostics(&built, goal)[0];
        assert_eq!(d.code, Code::SynthesisBlocked);
        assert!(d.message.contains("`Double`"), "{}", d.message);
        assert!(d.notes.iter().any(|n| n.contains("VL0403")), "{:?}", d.notes);
    }
    assert_eq!(script.calls(), 2, "Double and AddOne only");
}

/// The call cap fails the goals that reach it with `VL0403`, and never the ones that already have an artifact
/// (R-SYNTH-21).
#[test]
fn r_synth_21_the_call_cap_stops_a_build() {
    let dir = project("cap");
    let options = SynthOptions {
        max_calls_per_build: 1,
        ..SynthOptions::default()
    };
    let built = build_with(&dir, &source("Double it."), Some(&full_script()), options);
    assert_eq!(status(&built, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(diagnostics(&built, "AddOne")[0].code, Code::SynthesisFailed);
    assert!(diagnostics(&built, "AddOne")[0].message.contains("limit of 1"));
    assert_eq!(built.report.summary.calls, 1);
}

/// A watchdog stop during verification ends the goal with `VL0603` after one call, storing nothing, and its parent gets
/// `VL0409` (AC-SYNTH-35, D-93).
#[test]
fn ac_synth_35_a_watchdog_stop_is_not_a_rejection() {
    #[derive(Debug)]
    struct Jumping(AtomicU64);
    impl Clock for Jumping {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.fetch_add(1000, Ordering::SeqCst))
        }
    }
    let dir = project("watchdog");
    let text = source("Double it.");
    let program = program(&text);
    let script = replies(&[double(), add_one()]);
    let mut contacts = 0;
    let input = BuildInput {
        program: &program,
        source: &text,
        project: &dir,
        file: FILE,
        backend: Some(&script),
        options: SynthOptions::default(),
        run: Options {
            clock: Arc::new(Jumping(AtomicU64::new(0))),
            ..Options::default()
        },
    };
    let report = build(&input, &mut || contacts += 1);
    let goal = |name: &str| report.goals.iter().find(|g| g.goal == name).expect("goal");
    assert_eq!(goal("Double").diagnostics[0].code, Code::Timeout);
    assert_eq!(goal("Sum").status, Status::Blocked);
    assert_eq!(script.calls(), 2, "{:?}", report.goals);
    assert!(
        Lock::read(&dir)
            .expect("lock")
            .is_none_or(|l| l.entry(FILE, "Double").is_none())
    );
}

/// A store that can't be written is a diagnostic, never a panic (`VL0901`).
#[test]
fn a_store_that_cannot_be_written_is_a_diagnostic() {
    let dir = project("unwritable");
    fs::write(dir.join(".velme"), "not a directory").expect("a file in the way");
    let script = full_script();
    let built = build_with(&dir, &source("Double it."), Some(&script), SynthOptions::default());
    assert_eq!(diagnostics(&built, "Double")[0].code, Code::FileError);
    assert_eq!(status(&built, "Sum"), Status::Blocked);
}

/// Every file under `dir`, with its text where it is text.
fn files_under(dir: &Path, found: &mut Vec<(PathBuf, String)>) {
    for entry in fs::read_dir(dir).expect("a directory").filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, found);
        } else {
            let text = String::from_utf8_lossy(&fs::read(&path).expect("a file")).into_owned();
            found.push((path, text));
        }
    }
}

/// A recorded build through the `anthropic` provider with a sentinel key leaves the key in no lock, artifact, fixture or
/// log, and not in the report either (AC-SYNTH-09, AC-SEC-05, R-SEC-06).
#[test]
fn ac_synth_09_a_recorded_anthropic_build_leaks_no_key() {
    const SENTINEL: &str = "sk-ant-SENTINEL-KEY-0123456789";
    let dir = project("sentinel");
    let server = MockServer::start(
        [double(), add_one(), sum()]
            .map(|reply| MockResponse::tool_call("write_goal", &serde_json::from_str::<Value>(&reply).expect("JSON"))),
    );
    let sleeper = RecordingSleeper::default();
    let anthropic = mock_anthropic(AnthropicConfig::new("claude-test"), &server, SENTINEL, &sleeper);
    let recorder = Recorder::new(Box::new(anthropic), dir.join("tests/fixtures/synth"));
    let built = build_with(&dir, &source("Double it."), Some(&recorder), SynthOptions::default());
    assert_eq!(built.report.summary.calls, 3, "{:?}", built.report.goals);
    assert_eq!(server.requests()[0].headers["x-api-key"], SENTINEL, "the key was sent");
    let mut found = Vec::new();
    files_under(&dir, &mut found);
    assert!(
        found.iter().any(|(p, _)| p.ends_with("replay.json")) && found.len() >= 6,
        "{found:?}"
    );
    for (path, text) in &found {
        assert!(
            !text.contains(SENTINEL) && !text.contains("SENTINEL"),
            "{}",
            path.display()
        );
    }
    let shown = format!("{:?}", built.report);
    assert!(!shown.contains("SENTINEL"), "{shown}");
}

/// A replayed build reproduces a recorded one byte for byte: the same artifacts and the same lock (AC-SYNTH-10).
#[test]
fn ac_synth_10_replay_reproduces_a_recorded_build() {
    let text = source("Double it.");
    let fixtures = project("fixtures");
    let recorded = project("recorded");
    let recorder = Recorder::new(Box::new(replies(&[double(), add_one(), sum()])), &fixtures);
    let first = build_with(&recorded, &text, Some(&recorder), SynthOptions::default());
    assert_eq!(first.report.summary.calls, 3, "{:?}", first.report.goals);
    let replayed = project("replayed");
    let replay = Replay::new(&fixtures);
    let second = build_with(&replayed, &text, Some(&replay), SynthOptions::default());
    assert_eq!(second.report.summary.calls, 3, "{:?}", second.report.goals);
    assert_eq!(snapshot(&replayed), snapshot(&recorded));
}

/// A replay asks and reads as the recording did: the settings that shape requests and replies are in `replay.json`, and
/// applied over the local defaults, a recording made with `compact` replies replays to the same artifacts and lock. Under
/// the defaults it would read the compact replies as canonical IR and fail (R-SYNTH-43).
#[test]
fn r_synth_43_a_replay_follows_the_recorded_reply_format_and_history() {
    let text = source("Double it.");
    let (fixtures, recorded, replayed, defaults) = (
        project("options-fixtures"),
        project("options-recorded"),
        project("options-replayed"),
        project("options-defaults"),
    );
    let compact = SynthOptions {
        reply_format: velme_synth::ReplyFormat::Compact,
        retry_history: velme_synth::RetryHistory::All,
        ..SynthOptions::default()
    };
    let short =
        |reply: String| velme_synth::compress(&serde_json::from_str::<Value>(&reply).expect("JSON")).to_string();
    let scripted = replies(&[short(double()), short(add_one()), short(sum())]);
    let recorder = Recorder::new(Box::new(scripted), &fixtures).with_options(&compact);
    let first = build_with(&recorded, &text, Some(&recorder), compact);
    assert_eq!(first.report.summary.calls, 3, "{:?}", first.report.goals);

    let identity = velme_synth::read_replay_identity(&fixtures)
        .expect("readable")
        .expect("a replay.json");
    let mut options = SynthOptions::default();
    identity.apply(&mut options);
    assert_eq!(options.reply_format, velme_synth::ReplyFormat::Compact);
    assert_eq!(options.retry_history, velme_synth::RetryHistory::All);
    let replay = Replay::new(&fixtures);
    let second = build_with(&replayed, &text, Some(&replay), options);
    assert_eq!(second.report.summary.calls, 3, "{:?}", second.report.goals);
    assert_eq!(snapshot(&replayed), snapshot(&recorded));

    let third = build_with(&defaults, &text, Some(&replay), SynthOptions::default());
    assert_ne!(status(&third, "Double"), Status::Built(Source::Synthesized));
}

/// Goals are built in a topological order that takes the ready goal with the lowest source index next: `P` calls `B`
/// then `A`, and `A` and `B` follow it in the file, so the order is `A`, `B`, `P` (D-93, R-SYNTH-01).
#[test]
fn r_synth_01_goals_are_built_in_source_order_among_the_ready_ones() {
    let text = "language: velme/0.1

goal P(n: Number) -> Number:
    call:
        b = B(n)
        result = A(n)

goal A(n: Number) -> Number:
    plan: \"Add one.\"

goal B(n: Number) -> Number:
    plan: \"Double it.\"
";
    let dir = project("order");
    let program = program(text);
    install(
        &dir,
        FILE,
        &program,
        &ir(
            "A",
            &json!({"kind": "binary", "op": "add", "left": input(), "right": literal(1)}),
        ),
    );
    install(
        &dir,
        FILE,
        &program,
        &ir(
            "B",
            &json!({"kind": "binary", "op": "mul", "left": input(), "right": literal(2)}),
        ),
    );
    let built = build_with(&dir, text, None, SynthOptions::default());
    let order: Vec<&str> = built.report.goals.iter().map(|g| g.goal.as_str()).collect();
    assert_eq!(order, ["A", "B", "P"]);
    assert_eq!(status(&built, "P"), Status::Built(Source::Compiler));
}

/// A stored artifact found by its key is checked against a changed child like the lock's (R-ART-22): one that no longer
/// passes is not taken, and the goal is written again with the old IR and the failure in its first request (R-SYNTH-46).
#[test]
fn ac_art_10_a_store_hit_is_checked_against_a_changed_child() {
    let dir = project("store-reverify");
    build_with(
        &dir,
        &source("Double it."),
        Some(&full_script()),
        SynthOptions::default(),
    );
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let script = replies(&[double_off_at_three(), sum_alone()]);
    let built = build_with(&dir, &source("Twice."), Some(&script), SynthOptions::default());
    assert_eq!(status(&built, "Sum"), Status::Built(Source::Synthesized));
    assert_eq!(status(&built, "AddOne"), Status::Built(Source::Store));
    let requests = script.requests();
    assert_eq!(requests[1].attempts[0].diagnostics[0].code, "VL0503");
    let sum = built.report.goals.iter().find(|g| g.goal == "Sum").expect("goal");
    assert!(
        sum.notes
            .iter()
            .any(|n| n.starts_with("a stored version of `Sum` didn't pass after `Double` changed")),
        "{:?}",
        sum.notes
    );
}

/// A store hit is verified before it is pinned, even for a goal with no calls: an artifact planted under the goal's
/// `synthesis_key` whose hashes all agree but whose behaviour is wrong is rejected, not pinned (D-46, T-12, R-ART-22).
#[test]
fn ac_art_10_a_planted_store_hit_with_wrong_behaviour_is_rejected() {
    let dir = project("planted");
    let text = source("Double it.");
    build_with(&dir, &text, Some(&full_script()), SynthOptions::default());
    let store = velme_runtime::Store::new(&dir);
    let good = Lock::read(&dir)
        .expect("lock")
        .expect("a lock")
        .entry(FILE, "Double")
        .expect("entry")
        .artifact;
    // The same artifact with `Double` tripling: still canonical, still hashing to its own name, keyed as the real one.
    let mut planted = serde_json::to_value(store.get(good).expect("artifact")).expect("value");
    planted["ir"]["body"]["right"] = literal(3);
    let bytes = velme_ir::to_canonical_string(&planted).expect("canonical");
    let id = velme_ir::Fingerprint::of_bytes(bytes.as_bytes());
    assert_ne!(id, good);
    fs::remove_file(store.path(good)).expect("original removed");
    fs::write(store.path(id), &bytes).expect("planted");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let script = replies(&[double()]);
    let built = build_with(&dir, &text, Some(&script), SynthOptions::default());
    assert_eq!(status(&built, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(script.calls(), 1);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert_eq!(lock.entry(FILE, "Double").expect("entry").artifact, good);
}

/// A parent that could not be rebuilt after its child changed loses its lock entry (AC-SYNTH-03, R-ART-22, D-100), so
/// `velme run` finds it missing and asks for a build, and the next build, with no provider, says the same.
#[test]
fn ac_art_10_a_failed_rebuild_drops_the_parent_entry_and_run_asks_for_a_build() {
    let dir = project("stale-parent");
    build_with(
        &dir,
        &source("Double it."),
        Some(&full_script()),
        SynthOptions::default(),
    );
    let script = Scripted::new([Step::Reply(double_off_at_three()), Step::Reply("nope".to_owned())]);
    let options = SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    let text = source("Twice.");
    let second = build_with(&dir, &text, Some(&script), options);
    assert_eq!(diagnostics(&second, "Sum")[0].code, Code::SynthesisFailed);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert_eq!(lock.entry(FILE, "Sum"), None);
    assert!(lock.entry(FILE, "Double").is_some());
    let program = program(&text);
    let id = program
        .goals
        .iter()
        .position(|g| g.name == "Sum")
        .map(velme_sema::hir::GoalId)
        .expect("Sum");
    let Err(error) = velme_runtime::Registry::load(&program, id, FILE, &lock, &velme_runtime::Store::new(&dir)) else {
        panic!("a goal without an entry must not load");
    };
    assert_eq!(error[0].code, Code::LockStale);
    let third = build_with(&dir, &text, None, SynthOptions::default());
    assert_eq!(status(&third, "Sum"), Status::Failed);
    assert_eq!(diagnostics(&third, "Sum")[0].code, Code::ProviderUnavailable);
}

/// A leaf that is a lock hit is checked again on every build with no provider call, and one that no longer passes is
/// synthesized in the ordinary way (R-ART-22, D-100).
#[test]
fn ac_art_10_a_locked_leaf_is_verified_again_and_a_failing_one_is_synthesized() {
    let dir = project("leaf-reverify");
    let text = source("Double it.");
    build_with(&dir, &text, Some(&full_script()), SynthOptions::default());
    let store = velme_runtime::Store::new(&dir);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let good = lock.entry(FILE, "Double").expect("entry").artifact;
    // The stored file is replaced by a wrong artifact and the lock is pointed at it, keys unchanged.
    let mut planted = serde_json::to_value(store.get(good).expect("artifact")).expect("value");
    planted["ir"]["body"]["right"] = literal(3);
    let bytes = velme_ir::to_canonical_string(&planted).expect("canonical");
    let id = velme_ir::Fingerprint::of_bytes(bytes.as_bytes());
    fs::remove_file(store.path(good)).expect("original removed");
    fs::write(store.path(id), &bytes).expect("planted");
    let mut edited = lock.clone();
    let mut entry = lock.entry(FILE, "Double").expect("entry").clone();
    entry.artifact = id;
    edited.insert(entry);
    edited.write(&dir).expect("lock written");
    let script = replies(&[double()]);
    let built = build_with(&dir, &text, Some(&script), SynthOptions::default());
    assert_eq!(status(&built, "Double"), Status::Built(Source::Synthesized));
    assert_eq!(script.calls(), 1);
    let double = built.report.goals.iter().find(|g| g.goal == "Double").expect("goal");
    assert!(
        double
            .notes
            .iter()
            .any(|n| n.starts_with("the locked version of `Double` was checked again, and no longer passes: ")),
        "{:?}",
        double.notes
    );
}

/// Three levels: `Double` changes, `Sum` fails to verify and to rebuild, `Top` is blocked. `Sum` loses its entry, `Top`
/// keeps its own, and running `Top` fails naming `Sum` (R-ART-22, D-100).
#[test]
fn ac_art_10_only_the_rejected_goal_loses_its_entry_and_run_names_it() {
    let dir = project("three-levels");
    let text = format!(
        "{}\ngoal Top(n: Number) -> Number:\n    call:\n        result = Sum(n)\n",
        source("Double it.")
    );
    build_with(&dir, &text, Some(&full_script()), SynthOptions::default());
    let text = text.replace("Double it.", "Twice.");
    let script = Scripted::new([Step::Reply(double_off_at_three()), Step::Reply("nope".to_owned())]);
    let options = SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    let built = build_with(&dir, &text, Some(&script), options);
    assert_eq!(status(&built, "Sum"), Status::Failed);
    assert_eq!(status(&built, "Top"), Status::Blocked);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert!(lock.entry(FILE, "Sum").is_none());
    assert!(lock.entry(FILE, "Top").is_some());
    let program = program(&text);
    let id = program
        .goals
        .iter()
        .position(|g| g.name == "Top")
        .map(velme_sema::hir::GoalId)
        .expect("Top");
    let Err(error) = velme_runtime::Registry::load(&program, id, FILE, &lock, &velme_runtime::Store::new(&dir)) else {
        panic!("Top must not load");
    };
    assert!(
        error.iter().any(|d| d.message.contains("`Sum` has no verified build")),
        "{error:?}"
    );
}

/// A watchdog stop while a locked goal is checked again fails the goal with `VL0603` and keeps its entry: the lock never
/// depends on timing (INV-3, D-100).
#[test]
fn ac_art_10_a_watchdog_stop_on_a_lock_hit_keeps_the_entry() {
    #[derive(Debug)]
    struct Jumping(AtomicU64);
    impl Clock for Jumping {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.fetch_add(1000, Ordering::SeqCst))
        }
    }
    let dir = project("watchdog-lock-hit");
    let text = source("Double it.");
    build_with(&dir, &text, Some(&full_script()), SynthOptions::default());
    let before = Lock::read(&dir).expect("lock").expect("a lock");
    let program = program(&text);
    let mut contacts = 0;
    let input = BuildInput {
        program: &program,
        source: &text,
        project: &dir,
        file: FILE,
        backend: None,
        options: SynthOptions::default(),
        run: Options {
            clock: Arc::new(Jumping(AtomicU64::new(0))),
            ..Options::default()
        },
    };
    let report = build(&input, &mut || contacts += 1);
    let double = report.goals.iter().find(|g| g.goal == "Double").expect("goal");
    assert_eq!(double.diagnostics[0].code, Code::Timeout);
    let after = Lock::read(&dir).expect("lock").expect("a lock");
    assert_eq!(after.entry(FILE, "Double"), before.entry(FILE, "Double"));
}

/// After one `VL0404` the provider is not called again: the next goal that needs it ends the same way with no request
/// (AC-SYNTH-39, R-SYNTH-45).
#[test]
fn ac_synth_39_the_second_goal_gets_vl0404_without_a_request() {
    let dir = project("unavailable");
    let script = Scripted::new([
        Step::Error(ProviderError::Unavailable("down".to_owned())),
        Step::Reply(add_one()),
    ]);
    let built = build_with(&dir, &source("Double it."), Some(&script), SynthOptions::default());
    for goal in ["Double", "AddOne"] {
        let d = &diagnostics(&built, goal)[0];
        assert_eq!(d.code, Code::ProviderUnavailable, "{goal}");
        assert_eq!(
            d.message,
            format!("Velme couldn't reach the AI helper to build `{goal}`.")
        );
    }
    assert_eq!(script.calls(), 1);
}

/// A stored version that fails is a note of its own wording, and the note goes when a later stored version is taken:
/// the learner hears only about what still matters (R-SYNTH-46).
#[test]
fn ac_art_10_a_rejected_store_hit_leaves_no_note_when_another_is_taken() {
    let dir = project("planted-then-good");
    let text = source("Double it.");
    build_with(&dir, &text, Some(&full_script()), SynthOptions::default());
    let store = velme_runtime::Store::new(&dir);
    let good = Lock::read(&dir)
        .expect("lock")
        .expect("a lock")
        .entry(FILE, "Double")
        .expect("entry")
        .artifact;
    // A wrong artifact under the same key whose name sorts before the good one, so it is tried first.
    let genuine = serde_json::to_value(store.get(good).expect("artifact")).expect("value");
    let (id, bytes) = (3..64)
        .find_map(|k| {
            let mut planted = genuine.clone();
            planted["ir"]["body"]["right"] = literal(k);
            let bytes = velme_ir::to_canonical_string(&planted).expect("canonical");
            let id = velme_ir::Fingerprint::of_bytes(bytes.as_bytes());
            (id.hex() < good.hex()).then_some((id, bytes))
        })
        .expect("a wrong artifact that sorts first");
    fs::write(store.path(id), &bytes).expect("planted");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let none = replies(&[]);
    let built = build_with(&dir, &text, Some(&none), SynthOptions::default());
    assert_eq!(none.calls(), 0);
    assert_eq!(status(&built, "Double"), Status::Built(Source::Store));
    let double = built.report.goals.iter().find(|g| g.goal == "Double").expect("goal");
    assert!(double.notes.is_empty(), "{:?}", double.notes);
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    assert_eq!(lock.entry(FILE, "Double").expect("entry").artifact, good);
}
