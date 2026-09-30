//! The retry loop, the verification pipeline and the `VL0403` summary (`compiler/22` §5, §6, R-SYNTH-31..33).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::future::Future;

use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_diagnostics::Code;
use velme_ir::{Fingerprint, IR_VERSION, contract_key};
use velme_synth::{
    AttemptDiagnostic, AttemptFeedback, Outcome, ProviderError, ReplyFormat, RetryHistory, Scripted, Session, Sleeper,
    Step, SynthLimits, SynthOptions, SynthProvider, SynthReply, SynthRequest, Task, compress, render_with, synthesize,
    unavailable, with_transport_retries,
};
use velme_test_support::{LeafRunner, RecordingSleeper, goal_id, program};

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    check:
        - result == n * 2
    examples:
        - Double(2) == 4
        - Double(0) == 0
        - Double(3) == 6
";

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(future)
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

fn ir(body: &Value) -> String {
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "Double", "types": {},
           "inputs": [["n", number()]], "output": number(), "body": body})
    .to_string()
}

fn double() -> Value {
    json!({"kind": "binary", "op": "mul", "left": input(), "right": literal(2)})
}

/// `n * 2`, except that it is 3 at `n` (which the check `result == n * 2` catches, the examples not).
fn wrong_at(n: i64) -> String {
    ir(&json!({"kind": "if",
        "cond": {"kind": "binary", "op": "eq", "left": input(), "right": literal(n)},
        "then": literal(3), "else": double()}))
}

/// Fails the example `Double(2)` only.
fn wrong_example_two() -> String {
    ir(&literal(5))
}

/// Fails the example `Double(0)`, and passes `Double(2)`.
fn wrong_example_zero() -> String {
    ir(&json!({"kind": "binary", "op": "add", "left": input(), "right": literal(2)}))
}

/// A `call` node, which a candidate may not have (INV-6); the goal it names is the provider's text.
fn call_to(goal: &str) -> String {
    ir(&json!({"kind": "call", "binding": "x", "goal": goal, "goal_signature": "b3:00", "args": []}))
}

fn call_node() -> String {
    call_to("SENTINEL-GOAL")
}

fn good() -> String {
    ir(&double())
}

struct Run {
    outcome: Outcome,
    provider: Scripted,
    session: Session,
}

fn task(program: &velme_sema::hir::Program) -> Task<'_> {
    let id = goal_id(program, "Double");
    let contract = contract_key(program, id).expect("a key");
    Task {
        program,
        goal: id,
        source: SOURCE,
        contract_key: contract,
        synthesis_key: Fingerprint::of_bytes(b"synthesis"),
        feedback: Vec::new(),
    }
}

fn run_with(options: SynthOptions, steps: Vec<Step>, runner: &LeafRunner) -> Run {
    let program = program(SOURCE);
    let provider = Scripted::new(steps);
    let mut session = Session::new(options);
    let outcome = block_on(synthesize(&mut session, &provider, &task(&program), runner));
    Run {
        outcome,
        provider,
        session,
    }
}

fn run(options: SynthOptions, replies: Vec<String>) -> Run {
    run_with(
        options,
        replies.into_iter().map(Step::Reply).collect(),
        &LeafRunner::new(),
    )
}

fn failure(run: &Run) -> &velme_diagnostics::Diagnostic {
    match &run.outcome {
        Outcome::Failed(failure) => &failure.diagnostic,
        Outcome::Built(_) => panic!("it was built"),
    }
}

fn shown(d: &velme_diagnostics::Diagnostic) -> String {
    format!("{} | {} | {:?} | {:?}", d.code.as_str(), d.message, d.notes, d.help)
}

/// Two invalid replies with different causes, then a valid one: built after two retries, each retry carrying what was
/// found before (AC-SYNTH-02, AC-SYNTH-04).
#[test]
fn ac_synth_02_retry_turns_carry_the_previous_diagnostics() {
    let run = run(SynthOptions::default(), vec![call_node(), wrong_example_two(), good()]);
    assert!(matches!(run.outcome, Outcome::Built(_)));
    let requests = run.provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].attempts.is_empty());
    assert_eq!(requests[1].attempts.len(), 1);
    // A candidate with a `call` node is refused by the validator, and the retry turn cites `VL0402` (AC-SYNTH-04).
    assert_eq!(requests[1].attempts[0].diagnostics[0].code, "VL0402");
    assert_eq!(requests[1].attempts[0].reply, call_node());
    // The second failure is a failed example, wrapped as `VL0503` (R-SYNTH-15).
    let last = requests[2].attempts.last().expect("an attempt");
    assert_eq!(last.diagnostics[0].code, "VL0503");
    assert!(last.diagnostics[0].message.contains("gave 5"), "{last:?}");
    assert_eq!(run.session.calls(), 3);
    // The loop's attempt counter reaches the provider, which `retry_model` keys on (R-SYNTH-39).
    let attempts: Vec<u32> = run.provider.limits().iter().map(|l| l.attempt).collect();
    assert_eq!(attempts, [0, 1, 2]);
}

/// Four invalid replies, no two in a row with the same cause: `VL0403`, and the reply text of the provider appears
/// nowhere in it (AC-SYNTH-03, R-SYNTH-22).
#[test]
fn ac_synth_03_four_invalid_replies_end_in_vl0403() {
    let run = run(
        SynthOptions::default(),
        vec![
            call_node(),
            wrong_example_two(),
            wrong_example_zero(),
            "{\"SENTINEL\": 1}".to_owned(),
        ],
    );
    let d = failure(&run);
    assert_eq!(d.code, Code::SynthesisFailed);
    assert_eq!(run.session.calls(), 4);
    assert_eq!(run.provider.remaining(), 0);
    assert!(
        d.message.contains("4 of 4 tries") || d.message.contains("1 of 4 tries"),
        "{}",
        d.message
    );
    assert!(!format!("{d:?}").contains("SENTINEL"), "{d:?}");
}

/// A candidate that fails an example is rejected with the example's cause and never becomes a result (AC-SYNTH-05).
#[test]
fn ac_synth_05_a_failed_example_is_rejected() {
    let run = run(
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
        vec![wrong_example_two()],
    );
    let d = failure(&run);
    assert_eq!(d.code, Code::SynthesisFailed);
    assert!(
        d.message
            .contains("For Double(2), `Double` gave 5 but the example expects 4."),
        "{}",
        d.message
    );
    assert!(d.help.as_deref().is_some_and(|h| h.contains("check the example")));
}

/// With `stop_on_repeat = false`, attempts 1, 2 and 4 fail the same check on different inputs and attempt 3 an example:
/// the check with attempt 4's counterexample and "3 of 4", and one note for the example (AC-SYNTH-20).
#[test]
fn ac_synth_20_the_most_common_cause_is_named_with_the_latest_details() {
    let options = SynthOptions {
        stop_on_repeat: false,
        ..SynthOptions::default()
    };
    let run = run(
        options,
        vec![wrong_at(1), wrong_at(-1), wrong_example_two(), wrong_at(1_000_000)],
    );
    let d = failure(&run);
    assert!(
        d.message
            .contains("your check `result == n * 2` fails when `n` is `1000000`"),
        "{}",
        d.message
    );
    assert!(d.message.contains("3 of 4"), "{}", d.message);
    assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
    assert!(d.notes[0].contains("Double(2)"), "{:?}", d.notes);
}

/// Two causes twice each, alternating: the cause of the latest attempt (AC-SYNTH-21).
#[test]
fn ac_synth_21_a_tie_goes_to_the_later_attempt() {
    let run = run(
        SynthOptions::default(),
        vec![
            wrong_example_two(),
            wrong_example_zero(),
            wrong_example_two(),
            wrong_example_zero(),
        ],
    );
    let d = failure(&run);
    assert!(d.message.contains("For Double(0)"), "{}", d.message);
    assert!(d.message.contains("2 of 4"), "{}", d.message);
}

/// A question ends the goal at once, after one call, showing the question (R-SYNTH-32).
#[test]
fn r_synth_32_a_question_ends_synthesis_at_once() {
    let run = run(
        SynthOptions::default(),
        vec![r#"{"question": "Round up?"}"#.to_owned(), good()],
    );
    let d = failure(&run);
    assert_eq!(d.code, Code::PlanUnclear);
    assert!(d.notes.iter().any(|n| n.contains("Round up?")), "{:?}", d.notes);
    assert!(
        d.message.starts_with("Velme needs more detail to build `Double`."),
        "{}",
        d.message
    );
    assert_eq!(run.session.calls(), 1);
    assert_eq!(run.provider.remaining(), 1);
}

/// A question's newline and ANSI escape become spaces; an empty one and a 281-character one are failed attempts
/// (AC-SYNTH-23, R-SYNTH-33).
#[test]
fn ac_synth_23_a_question_is_cleaned_and_bounded() {
    let messy = json!({"question": "Round\nup\u{1b}[31m red\u{1b}[0m or\tdown?"}).to_string();
    let run = run(SynthOptions::default(), vec![messy]);
    let d = failure(&run);
    assert!(
        d.notes.iter().any(|n| n.contains("\"Round up red or down?\"")),
        "{:?}",
        d.notes
    );
    for question in [String::new(), "x".repeat(281)] {
        let reply = json!({ "question": question }).to_string();
        let run = self::run(
            SynthOptions {
                max_retries: 0,
                ..SynthOptions::default()
            },
            vec![reply],
        );
        let d = failure(&run);
        assert_eq!(d.code, Code::SynthesisFailed, "{}", shown(d));
        assert!(d.message.contains("question"), "{}", d.message);
    }
    let edge = json!({"question": "x".repeat(280)}).to_string();
    assert_eq!(
        failure(&self::run(SynthOptions::default(), vec![edge])).code,
        Code::PlanUnclear
    );
}

/// A compact reply expands to the same IR as the same reply in `ir-json`; an unknown alias is a failed attempt
/// (AC-SYNTH-26).
#[test]
fn ac_synth_26_a_compact_reply_expands_to_canonical_ir() {
    let canonical: Value = serde_json::from_str(&good()).expect("json");
    let compact = compress(&canonical).to_string();
    assert_ne!(compact, canonical.to_string());
    let plain = run(SynthOptions::default(), vec![good()]);
    let options = SynthOptions {
        reply_format: ReplyFormat::Compact,
        ..SynthOptions::default()
    };
    let packed = self::run(options.clone(), vec![compact]);
    let (Outcome::Built(a), Outcome::Built(b)) = (&plain.outcome, &packed.outcome) else {
        panic!("both build");
    };
    assert_eq!(a.ir, b.ir);
    let bad = self::run(
        SynthOptions {
            max_retries: 0,
            ..options
        },
        vec![r#"{"zz": 1}"#.to_owned()],
    );
    let d = failure(&bad);
    assert_eq!(d.code, Code::SynthesisFailed);
    assert!(d.message.contains("short name"), "{}", d.message);
}

/// With `latest` the third request carries reply 2 and the primary diagnostics of attempts 1 and 2; with `all`, both
/// replies (AC-SYNTH-27).
#[test]
fn ac_synth_27_retry_history_latest_and_all() {
    let replies = || vec![call_node(), wrong_example_two(), wrong_example_zero(), good()];
    let latest = run(SynthOptions::default(), replies());
    let third = &latest.provider.requests()[2];
    assert_eq!(third.attempts.len(), 2);
    assert_eq!(third.attempts[0].reply, "");
    assert_eq!(third.attempts[0].diagnostics.len(), 1);
    assert_eq!(third.attempts[1].reply, wrong_example_two());
    let all = run(
        SynthOptions {
            retry_history: RetryHistory::All,
            ..SynthOptions::default()
        },
        replies(),
    );
    let third = &all.provider.requests()[2];
    assert_eq!(third.attempts[0].reply, call_node());
    assert_eq!(third.attempts[1].reply, wrong_example_two());
}

/// Two replies failing the same check stop the loop after 2 calls; without `stop_on_repeat`, 4 calls (AC-SYNTH-28).
#[test]
fn ac_synth_28_a_repeated_cause_stops_the_loop() {
    let run1 = run(
        SynthOptions::default(),
        vec![wrong_at(1), wrong_at(-1), wrong_at(1), wrong_at(-1)],
    );
    let d = failure(&run1);
    assert_eq!(run1.session.calls(), 2);
    assert!(d.message.contains("stopped after 2 attempts"), "{}", d.message);
    assert!(d.message.contains("result == n * 2"), "{}", d.message);
    let options = SynthOptions {
        stop_on_repeat: false,
        ..SynthOptions::default()
    };
    let run2 = run(options, vec![wrong_at(1), wrong_at(-1), wrong_at(1), wrong_at(-1)]);
    assert_eq!(run2.session.calls(), 4);
}

/// With `max_prompt_examples = 2` the prompt shows two of three examples; a candidate failing the third is rejected and
/// the next retry turn shows it with its values (AC-SYNTH-29).
#[test]
fn ac_synth_29_examples_left_out_of_the_prompt_still_run() {
    let options = SynthOptions {
        max_prompt_examples: 2,
        ..SynthOptions::default()
    };
    let failing_third = ir(&json!({"kind": "if",
        "cond": {"kind": "binary", "op": "eq", "left": input(), "right": literal(3)},
        "then": literal(0), "else": double()}));
    let run = run(options.clone(), vec![failing_third, good()]);
    assert!(matches!(run.outcome, Outcome::Built(_)));
    let requests = run.provider.requests();
    let first = render_with(&requests[0], &options.prompt()).expect("a prompt");
    assert!(first.first_turn().contains("Double(2) == 4") && first.first_turn().contains("Double(0) == 0"));
    assert!(!first.first_turn().contains("Double(3) == 6"));
    let second = render_with(&requests[1], &options.prompt()).expect("a prompt");
    let retry = &second.retries[1].text;
    assert!(
        retry.contains("For Double(3), `Double` gave 0 but the example expects 6"),
        "{retry}"
    );
    assert!(retry.contains("input: {\"n\":3}"), "{retry}");
}

/// Options that change what is sent change `input_version`, so `synthesis_key` (R-SYNTH-40).
#[test]
fn r_synth_40_options_enter_the_input_version() {
    let base = SynthOptions::default();
    let version = base.input_version("prompt-1:x");
    assert!(version.starts_with("prompt-1:x+b3:"));
    for other in [
        SynthOptions {
            max_prompt_examples: 2,
            ..SynthOptions::default()
        },
        SynthOptions {
            reply_format: ReplyFormat::Compact,
            ..SynthOptions::default()
        },
        SynthOptions {
            retry_history: RetryHistory::All,
            ..SynthOptions::default()
        },
    ] {
        assert_ne!(other.input_version("prompt-1:x"), version);
    }
    // Settings that only bound the loop change no key.
    assert_eq!(
        SynthOptions {
            max_retries: 1,
            stop_on_repeat: false,
            ..SynthOptions::default()
        }
        .input_version("prompt-1:x"),
        version
    );
}

/// A watchdog `VL0603` in verification ends the goal after one call, never as a rejected candidate (AC-SYNTH-35, the
/// goal's half).
#[test]
fn r_synth_14_a_watchdog_stop_is_not_a_verdict() {
    let run = run_with(
        SynthOptions::default(),
        vec![Step::Reply(good()), Step::Reply(good())],
        &LeafRunner::timing_out_at(2),
    );
    let d = failure(&run);
    assert_eq!(d.code, Code::Timeout);
    assert_eq!(run.session.calls(), 1);
    assert_eq!(run.provider.remaining(), 1);
}

/// A provider that is unreachable: one call whose transport tries wait 1 s then 2 s on the injected clock, `VL0404`; the
/// next goal is not sent a request (AC-SYNTH-39, R-SYNTH-45).
#[test]
fn ac_synth_39_after_one_vl0404_the_provider_is_not_contacted_again() {
    struct Down {
        sleeper: RecordingSleeper,
        tries: std::sync::atomic::AtomicUsize,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SynthProvider for Down {
        fn id(&self) -> &str {
            "down"
        }
        fn model(&self) -> &str {
            "m"
        }
        fn input_version(&self) -> &str {
            "v"
        }
        async fn complete(&self, _: &SynthRequest, _: &SynthLimits) -> Result<SynthReply, ProviderError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            with_transport_retries(&self.sleeper as &dyn Sleeper, || async {
                self.tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(ProviderError::Unavailable("no route".to_owned()))
            })
            .await
        }
    }
    let program = program(SOURCE);
    let provider = Down {
        sleeper: RecordingSleeper::default(),
        tries: 0.into(),
        calls: 0.into(),
    };
    let mut session = Session::new(SynthOptions::default());
    let runner = LeafRunner::new();
    let first = block_on(synthesize(&mut session, &provider, &task(&program), &runner));
    let second = block_on(synthesize(&mut session, &provider, &task(&program), &runner));
    for outcome in [first, second] {
        let Outcome::Failed(failure) = outcome else {
            panic!("it fails")
        };
        assert_eq!(failure.diagnostic.code, Code::ProviderUnavailable);
    }
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(provider.tries.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(
        provider.sleeper.waits(),
        [std::time::Duration::from_secs(1), std::time::Duration::from_secs(2)]
    );
    assert!(session.unavailable());
    assert_eq!(
        unavailable("Double", Default::default(), None).message,
        "Velme couldn't reach the AI helper to build `Double`."
    );
    assert_eq!(
        unavailable("Double", Default::default(), None).code,
        Code::ProviderUnavailable
    );
}

/// A `RateLimited` waits its `retry_after`, at most 30 s (R-SYNTH-12).
#[test]
fn r_synth_12_retry_after_is_honoured_up_to_30_seconds() {
    let sleeper = RecordingSleeper::default();
    let mut answers = vec![
        ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(120)),
        },
        ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(5)),
        },
    ]
    .into_iter();
    let result: Result<u8, ProviderError> = block_on(with_transport_retries(&sleeper as &dyn Sleeper, || {
        let next = answers.next();
        async move { next.map_or(Ok(7), Err) }
    }));
    assert_eq!(result, Ok(7));
    assert_eq!(
        sleeper.waits(),
        [std::time::Duration::from_secs(30), std::time::Duration::from_secs(5)]
    );
}

/// A pending reply ends the goal at once with `VL0408` and a note naming the earlier failed attempt (R-SYNTH-41).
#[test]
fn r_synth_41_pending_ends_the_goal_and_names_an_earlier_failure() {
    let steps = vec![
        Step::Reply(wrong_example_two()),
        Step::Error(ProviderError::Pending("ticket\n42".to_owned())),
        Step::Reply(good()),
    ];
    let run = run_with(SynthOptions::default(), steps, &LeafRunner::new());
    let d = failure(&run);
    assert_eq!(d.code, Code::SynthesisPending);
    assert!(d.notes.iter().any(|n| n.contains("\"ticket 42\"")), "{:?}", d.notes);
    assert!(d.notes.iter().any(|n| n.contains("Double(2)")), "{:?}", d.notes);
    assert_eq!(run.session.calls(), 2);
    let empty = run_with(
        SynthOptions::default(),
        vec![Step::Error(ProviderError::Pending(String::new()))],
        &LeafRunner::new(),
    );
    assert_eq!(failure(&empty).code, Code::BackendFailed);
}

/// Refused and malformed replies are failed attempts in Velme's words; the provider's text never appears (R-SYNTH-07).
#[test]
fn r_synth_07_refused_and_malformed_are_failed_attempts() {
    let steps = vec![
        Step::Error(ProviderError::Refused("SENTINEL refusal".to_owned())),
        Step::Error(ProviderError::Malformed("SENTINEL malformed".to_owned())),
        Step::Reply(good()),
    ];
    let run = run_with(SynthOptions::default(), steps, &LeafRunner::new());
    assert!(matches!(run.outcome, Outcome::Built(_)));
    let bad = run_with(
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
        vec![Step::Error(ProviderError::Refused("SENTINEL refusal".to_owned()))],
        &LeafRunner::new(),
    );
    let d = failure(&bad);
    assert_eq!(d.code, Code::SynthesisFailed);
    assert!(!format!("{d:?}").contains("SENTINEL"));
}

/// The call cap: the next call is refused with `VL0403` naming the limit (R-SYNTH-21).
#[test]
fn r_synth_21_the_call_cap_ends_a_goal_with_vl0403() {
    let options = SynthOptions {
        max_calls_per_build: 1,
        ..SynthOptions::default()
    };
    let run = run(options, vec![wrong_example_two(), good()]);
    let d = failure(&run);
    assert_eq!(d.code, Code::SynthesisFailed);
    assert!(d.message.contains("limit of 1 provider calls"), "{}", d.message);
    assert_eq!(run.session.calls(), 1);
}

/// Two attempts break the same validator rule while naming different things: they share a cause, so the loop stops after
/// two calls, and nothing the replies named reaches what the learner reads (R-SYNTH-22, R-SYNTH-31, D-95).
#[test]
fn r_synth_31_a_validator_cause_is_its_rule_not_its_message() {
    let run = run(
        SynthOptions::default(),
        vec![call_to("SENTINEL-ONE"), call_to("SENTINEL-TWO"), good()],
    );
    let d = failure(&run);
    assert_eq!(run.session.calls(), 2);
    assert!(
        d.message.contains("2 of 2 tries, stopped after 2 attempts"),
        "{}",
        d.message
    );
    let Outcome::Failed(failed) = &run.outcome else {
        panic!("it fails")
    };
    let shown = format!("{d:?} {:?}", failed.attempts);
    assert!(!shown.contains("SENTINEL"), "{shown}");
    assert!(failed.attempts[0].contains("rule structure-"), "{:?}", failed.attempts);
    // The provider still gets the whole story, path apart from detail.
    let sent = &run.provider.requests()[1].attempts[0].diagnostics[0];
    assert_eq!(sent.code, "VL0402");
    assert!(
        sent.path
            .as_deref()
            .is_some_and(|p| p.starts_with('/') && !p.contains('`')),
        "{sent:?}"
    );
    assert!(sent.detail.is_none(), "{sent:?}");
    // A reply that isn't JSON says so in Velme's words, not the parser's.
    let bad = self::run(
        SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
        vec!["{\"SENTINEL\": 1}".to_owned()],
    );
    let d = failure(&bad);
    assert!(
        d.message.contains("isn't valid JSON of the shape the IR needs"),
        "{}",
        d.message
    );
    assert!(!format!("{d:?}").contains("SENTINEL"));
}

/// A retry turn keeps the diagnostics of an attempt that has no reply (a refusal), after a reply-bearing one, and the
/// conversation still alternates (R-SYNTH-11).
#[test]
fn r_synth_11_feedback_of_a_reply_less_attempt_is_rendered() {
    let base = run(SynthOptions::default(), vec![good()]).provider.requests()[0].clone();
    let diagnostic = |marker: &str| AttemptDiagnostic {
        code: "VL0401".to_owned(),
        message: marker.to_owned(),
        path: None,
        detail: None,
    };
    let attempt = |reply: &str, marker: &str| AttemptFeedback {
        reply: reply.to_owned(),
        diagnostics: vec![diagnostic(marker)],
    };
    let check = |attempts: Vec<AttemptFeedback>, markers: &[&str]| {
        let mut request = base.clone();
        request.attempts = attempts;
        let turns = render_with(&request, &SynthOptions::default().prompt())
            .expect("a prompt")
            .turns();
        let all: String = turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join("\n");
        for marker in markers {
            assert!(all.contains(marker), "{marker} in {all}");
        }
        assert!(turns.windows(2).all(|w| w[0].role != w[1].role), "roles alternate");
    };
    check(vec![attempt("", "REFUSED-ONLY")], &["REFUSED-ONLY"]);
    check(
        vec![attempt("{\"a\":1}", "FIRST"), attempt("", "REFUSED-LAST")],
        &["FIRST", "REFUSED-LAST"],
    );
    // With `latest`, the request already carries reply-less earlier attempts and a reply-less last one.
    check(vec![attempt("", "OLD"), attempt("", "NEWER")], &["OLD", "NEWER"]);
}
