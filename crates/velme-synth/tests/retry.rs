//! The retry loop, the verification pipeline and the `VL0403` summary (`compiler/22` §5, §6, R-SYNTH-31..33).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::future::Future;

use serde_json::{Value, json};
use velme_diagnostics::Code;
use velme_ir::{Fingerprint, contract_key};
use velme_synth::{
    AttemptDiagnostic, AttemptFeedback, Outcome, ProviderError, ReplyFormat, RetryHistory, Scripted, Session, Sleeper,
    Step, SynthLimits, SynthOptions, SynthProvider, SynthReply, SynthRequest, Task, alias_table, compress, render_with,
    synthesize, unavailable, with_transport_retries,
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

/// A reply holding `body` (D-103).
fn reply(body: &Value) -> String {
    json!({ "body": body }).to_string()
}

fn double() -> Value {
    json!({"kind": "binary", "op": "mul", "left": input(), "right": literal(2)})
}

/// `n * 2`, except that it is 3 at `n` (which the check `result == n * 2` catches, the examples not).
fn wrong_at(n: i64) -> String {
    reply(&json!({"kind": "if",
        "cond": {"kind": "binary", "op": "eq", "left": input(), "right": literal(n)},
        "then": literal(3), "else": double()}))
}

/// Fails the example `Double(2)` only.
fn wrong_example_two() -> String {
    reply(&literal(5))
}

/// Fails the example `Double(0)`, and passes `Double(2)`.
fn wrong_example_zero() -> String {
    reply(&json!({"kind": "binary", "op": "add", "left": input(), "right": literal(2)}))
}

/// A `call` node, which a candidate may not have (INV-6); the goal it names is the provider's text.
fn call_to(goal: &str) -> String {
    reply(&json!({"kind": "call", "binding": "x", "goal": goal, "goal_signature": "b3:00", "args": []}))
}

fn call_node() -> String {
    call_to("SENTINEL-GOAL")
}

fn good() -> String {
    reply(&double())
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

/// A reply with a `call` node fails validation, and the retry turn cites `VL0402` (AC-SYNTH-04, INV-6).
#[test]
fn ac_synth_04_a_call_node_is_refused_and_the_retry_cites_vl0402() {
    let run = run(SynthOptions::default(), vec![call_node(), good()]);
    assert!(matches!(run.outcome, Outcome::Built(_)));
    let requests = run.provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].attempts[0].diagnostics[0].code, "VL0402");
    assert_eq!(requests[1].attempts[0].reply, call_node());
}

/// Only `body` is read from a reply: the goal's name, versions, `inputs`, `output` and `types` come from the request, so
/// a reply that spells any of them wrongly builds the same IR as one that leaves them out (AC-SYNTH-40, D-103).
#[test]
fn ac_synth_40_only_the_body_is_read_from_a_reply() {
    let noisy = json!({
        "body": double(), "goal": "Other", "ir_version": "9.9", "builtins_version": "9.9",
        "inputs": [["a", {"t": "Record", "name": "Number"}]], "output": {"t": "Text"},
        "types": {"Ghost": {"fields": []}},
    })
    .to_string();
    let plain = run(SynthOptions::default(), vec![good()]);
    let extra = run(SynthOptions::default(), vec![noisy]);
    let (Outcome::Built(a), Outcome::Built(b)) = (&plain.outcome, &extra.outcome) else {
        panic!("both build");
    };
    assert_eq!(a.ir, b.ir);
    assert_eq!(a.ir.goal().goal, "Double");
    assert_eq!(a.ir.goal().inputs.len(), 1);
    assert!(a.ir.goal().types.is_empty());
    // The same holds in the compact format: the body member is expanded alone, whatever else the reply holds.
    let options = SynthOptions {
        reply_format: ReplyFormat::Compact,
        ..SynthOptions::default()
    };
    let mut compact = compress(&serde_json::from_str::<Value>(&good()).expect("json"));
    compact["goal"] = json!("Other");
    compact["extra"] = json!({"not": "an alias"});
    let packed = run(options, vec![compact.to_string()]);
    let Outcome::Built(c) = &packed.outcome else {
        panic!("it builds")
    };
    assert_eq!(a.ir, c.ir);
}

/// A reply that is not JSON or holds no `body` is a failed attempt in Velme's words, and the help of `VL0403` follows the
/// kind of rule that failed: the builtins hint only where a built-in is at fault (AC-SYNTH-41, D-103).
#[test]
fn ac_synth_41_the_help_follows_the_kind_of_failure() {
    const FORM: &str = "the AI helper's reply wasn't in a form Velme can use: build again, or try another model";
    const FIT: &str = "the AI helper's code didn't fit the goal: build again, or add an example that shows the result";
    const BIG: &str = "the plan may be too big for one goal: simplify it or split the goal";
    const BUILTINS: &str = "the plan may need more than the listed builtins can do: simplify it or split the goal";
    let once = || SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    // The message, the first attempt's line and the help of the one attempt that `reply` is.
    let outcome = |reply: String| {
        let run = run(once(), vec![reply]);
        let d = failure(&run);
        let Outcome::Failed(failed) = &run.outcome else {
            panic!("it fails")
        };
        (
            d.message.clone(),
            failed.attempts[0].clone(),
            d.help.as_deref().expect("a help line").to_owned(),
        )
    };
    for (reply, words) in [
        ("not json".to_owned(), "the reply wasn't JSON"),
        (json!({"goal": "Double"}).to_string(), "the reply had no `body`"),
    ] {
        let (message, attempt, help) = outcome(reply);
        assert!(message.contains(words), "{message}");
        assert!(attempt.starts_with("attempt 1: VL0401: "), "{attempt}");
        assert_eq!(help, FORM);
    }
    // A body that is not an expression fails the schema; one whose result is the wrong type fails a type rule.
    let (_, attempt, help) = outcome(reply(&json!(5)));
    assert!(attempt.starts_with("attempt 1: VL0401: "), "{attempt}");
    assert_eq!(help, FORM);
    let (_, _, help) = outcome(reply(&literal_text()));
    assert_eq!(help, FIT);
    // A body over the size limit is about size.
    let huge = json!({"kind": "literal", "type": {"t": "Text"}, "value": "x".repeat(1_100_000)});
    let (_, attempt, help) = outcome(reply(&huge));
    assert!(attempt.contains("structure-1"), "{attempt}");
    assert_eq!(help, BIG);
    // A built-in that isn't there is the one case that is about what the builtins can do.
    let unknown = reply(&json!({"kind": "builtin", "name": "read_file", "args": [input()]}));
    let (message, _, help) = outcome(unknown);
    assert!(message.contains("uses a built-in that doesn't exist"), "{message}");
    assert_eq!(help, BUILTINS);
}

/// What a provider chose inside `body` never becomes a bug of ours: a number canonical JSON can't hold, a repeated key
/// and a reply that is both a body and a question are each a failed attempt with `VL0401`, worded by Velme
/// (AC-SYNTH-41, R-SYNTH-10, D-103).
#[test]
fn ac_synth_41_a_reply_body_that_cannot_be_read_is_a_failed_attempt() {
    let once = || SynthOptions {
        max_retries: 0,
        ..SynthOptions::default()
    };
    let failed = |reply: &str| {
        let run = self::run(once(), vec![reply.to_owned()]);
        let Outcome::Failed(failure) = &run.outcome else {
            panic!("it fails")
        };
        assert_eq!(
            failure.diagnostic.code,
            Code::SynthesisFailed,
            "{}",
            failure.diagnostic.message
        );
        (failure.diagnostic.message.clone(), failure.attempts[0].clone())
    };
    let (message, attempt) = failed(r#"{"body": {"kind": "literal", "type": {"t": "Number"}, "value": 1e99999}}"#);
    assert!(message.contains("holds a number Velme can't read"), "{message}");
    assert!(attempt.starts_with("attempt 1: VL0401: "), "{attempt}");
    // A repeated key is `schema-1` at its path under `/body`, not "wasn't JSON".
    let (message, attempt) = failed(r#"{"body": {"kind": "input", "name": "n", "name": "m"}}"#);
    assert!(
        message.contains("isn't valid JSON of the shape the IR needs at /body/name"),
        "{message}"
    );
    assert!(attempt.contains("(schema-1)"), "{attempt}");
    let (message, _) = failed(r#"{"body": {"kind": "input", "name": "n"}, "question": "Which way?"}"#);
    assert!(message.contains("held both a body and a question"), "{message}");
    // What the next request carries shows the repeated key's path and no position in a document the model never wrote.
    let run = self::run(
        SynthOptions {
            max_retries: 1,
            stop_on_repeat: false,
            ..SynthOptions::default()
        },
        vec![r#"{"body": {"kind": "input"}}"#.to_owned(), good()],
    );
    let sent = &run.provider.requests()[1].attempts[0].diagnostics[0];
    let text = format!("{sent:?}");
    assert!(!text.contains("column"), "{text}");
}

fn literal_text() -> Value {
    json!({"kind": "literal", "type": {"t": "Text"}, "value": "x"})
}

/// What the next request says to a reply that reads names or fields the goal doesn't have (AC-SYNTH-43, R-SYNTH-49,
/// D-104): the detail of the first diagnostic of each reply, after `replies` in turn, from the request alone.
fn hints(source: &str, goal: &str, format: ReplyFormat, replies: Vec<String>) -> Vec<Vec<Option<String>>> {
    let program = program(source);
    let id = goal_id(&program, goal);
    let task = Task {
        program: &program,
        goal: id,
        source,
        contract_key: contract_key(&program, id).expect("a key"),
        synthesis_key: Fingerprint::of_bytes(b"synthesis"),
        feedback: Vec::new(),
    };
    let count = replies.len() as u32;
    let options = SynthOptions {
        max_retries: count,
        stop_on_repeat: false,
        retry_history: RetryHistory::All,
        reply_format: format,
        ..SynthOptions::default()
    };
    let provider = Scripted::replies(replies.iter().cloned().chain([good()]));
    let mut session = Session::new(options);
    let _ = block_on(synthesize(&mut session, &provider, &task, &LeafRunner::new()));
    let sent = provider.requests();
    let last = &sent[replies.len()];
    last.attempts
        .iter()
        .map(|a| a.diagnostics.iter().map(|d| d.detail.clone()).collect())
        .collect()
}

/// A name or field the goal doesn't have is answered with what is there and which node reads it, in the reply's format,
/// once per attempt, and none of the reply's text comes back (AC-SYNTH-43, R-SYNTH-49, D-104).
#[test]
fn ac_synth_43_feedback_for_a_wrong_name_lists_what_is_available() {
    const RECORDS: &str = "language: velme/0.1

type Player:
    name: Text
    score: Number

goal Rank(player: Player, bonus: Number) -> Number:
    plan: \"Return the score plus the bonus.\"

goal Bump(n: Number) -> Number:
    plan: \"Add one.\"

goal Twice(n: Number) -> Number:
    call:
        b = Bump(n)
    plan: \"Bump the result again.\"
";
    let field = |name: &str| json!({"kind": "field", "of": {"kind": "input", "name": "player"}, "field": name});
    let local = |name: &str| json!({"kind": "local", "name": name});
    let text = |value: &str| json!({"kind": "literal", "type": {"t": "Text"}, "value": value});
    let binary = |op: &str, l: &Value, r: &Value| json!({"kind": "binary", "op": op, "left": l, "right": r});
    let inputs = "the goal's inputs, each read with an `input` node, are: player: Player, bonus: Number";
    let detail = |found: &Vec<Vec<Option<String>>>, n: usize| found[n][0].clone().unwrap_or_default();
    let ir = ReplyFormat::IrJson;
    let leaf = hints(
        RECORDS,
        "Rank",
        ir,
        vec![
            // An input written as a `local`; a dotted name; a field the record doesn't have; an input that isn't there.
            reply(&local("player")),
            reply(&local("SENTINEL.score")),
            reply(&field("SENTINEL")),
            reply(&json!({"kind": "input", "name": "SENTINEL"})),
            // `add` on Text points to `concat`; `gt` on Text says nothing of the kind.
            reply(&binary("add", &text("a"), &text("b"))),
            reply(&binary("gt", &text("a"), &literal(1))),
            // Two unknown inputs in one reply: the list is said once.
            reply(&binary(
                "add",
                &json!({"kind": "input", "name": "X"}),
                &json!({"kind": "input", "name": "Y"}),
            )),
        ],
    );
    assert_eq!(
        detail(&leaf, 0),
        format!("{inputs}; a `local` node reads only a call result or a lambda's parameter")
    );
    let dotted = detail(&leaf, 1);
    assert!(
        dotted.contains("a name is never dotted: read a field with a `field` node over the `input` node; ")
            && dotted.contains(inputs),
        "{dotted}"
    );
    assert_eq!(
        detail(&leaf, 2).split("; ").last(),
        Some("the fields of `Player` are: name: Text, score: Number")
    );
    assert!(detail(&leaf, 3).contains(inputs), "{}", detail(&leaf, 3));
    for n in 0..4 {
        // The hint is the request's own words: the message beside it may quote the reply, the hint never does.
        assert!(!detail(&leaf, n).contains("SENTINEL"), "{}", detail(&leaf, n));
    }
    assert!(
        detail(&leaf, 4).contains("join Text with the `concat` builtin"),
        "{}",
        detail(&leaf, 4)
    );
    assert!(!detail(&leaf, 5).contains("concat"), "{}", detail(&leaf, 5));
    assert_eq!(leaf[6].len(), 2);
    assert!(leaf[6][0].as_deref().unwrap_or_default().contains(inputs));
    assert!(
        !leaf[6][1].as_deref().unwrap_or_default().contains(inputs),
        "{:?}",
        leaf[6]
    );

    // A built-in that is an operator points to the node that writes it; any other name gets the list, said once.
    let builtin =
        |name: &str| reply(&json!({"kind": "builtin", "name": name, "args": [{"kind": "input", "name": "bonus"}]}));
    let named = hints(
        RECORDS,
        "Rank",
        ir,
        vec![
            builtin("add"),
            builtin("not"),
            builtin("SENTINEL"),
            reply(&binary(
                "add",
                &json!({"kind": "builtin", "name": "X", "args": []}),
                &json!({"kind": "builtin", "name": "Y", "args": []}),
            )),
        ],
    );
    assert_eq!(
        detail(&named, 0).split("; ").last(),
        Some("`add` is an operator, not a built-in: write a `binary` node with `op` `add`")
    );
    assert!(
        detail(&named, 1).contains("write a `unary` node with `op` `not`"),
        "{}",
        detail(&named, 1)
    );
    // Only what a `builtin` node can call: the collection primitives are nodes of their own.
    assert_eq!(
        detail(&named, 2).split("; ").last(),
        Some(
            "a `builtin` node names only a built-in of the list: length, is_empty, maximum, minimum, sum, contains, \
             abs, floor, ceil, round, clamp, concat, to_text, range, random"
        )
    );
    assert!(
        named[3][0]
            .as_deref()
            .unwrap_or_default()
            .contains("names only a built-in")
    );
    assert!(
        !named[3][1]
            .as_deref()
            .unwrap_or_default()
            .contains("names only a built-in"),
        "{:?}",
        named[3]
    );

    // A composite lists its call results too, and a dotted name that starts at one is a field over a `local`.
    let calls = "the goal's inputs, each read with an `input` node, are: n: Number, and the call results, each read with a `local` node, are: b: Number";
    let composite = hints(
        RECORDS,
        "Twice",
        ir,
        vec![
            reply(&json!({"kind": "input", "name": "SENTINEL"})),
            reply(&local("b.SENTINEL")),
            reply(&local("n.x")),
        ],
    );
    assert!(detail(&composite, 0).contains(calls), "{}", detail(&composite, 0));
    let over_local = detail(&composite, 1);
    assert!(
        over_local.contains("a `field` node over the `local` node; ") && over_local.contains(calls),
        "{over_local}"
    );
    assert!(
        detail(&composite, 2).contains("over the `input` node; "),
        "{}",
        detail(&composite, 2)
    );

    // In the compact format the nodes are named as the reply is asked to spell them.
    let compact = hints(
        RECORDS,
        "Twice",
        ReplyFormat::Compact,
        vec![compress(&json!({"body": {"kind": "input", "name": "SENTINEL"}})).to_string()],
    );
    let table = alias_table();
    let alias = &table.kinds["input"];
    assert!(
        detail(&compact, 0).contains(&format!("read with an `input` (`{alias}`) node")),
        "{}",
        detail(&compact, 0)
    );
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
        vec![compress(&json!({"body": {"zz": 1}})).to_string()],
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
    let failing_third = reply(&json!({"kind": "if",
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
    assert!(session.stopped().is_some());
    assert_eq!(
        unavailable("Double", Default::default(), None).message,
        "Velme couldn't reach the AI helper to build `Double`."
    );
    assert_eq!(
        unavailable("Double", Default::default(), None).code,
        Code::ProviderUnavailable
    );
}

/// A rejected request is `VL0405` naming the status, with the API's message as a note, not `VL0607`; it ends that goal
/// only, so the next goal still makes its request (R-SYNTH-07, R-SYNTH-45, D-150, AC-SYNTH-46).
#[test]
fn r_synth_07_a_rejected_request_is_vl0405_with_the_api_message() {
    let program = program(SOURCE);
    let provider = Scripted::new([
        Step::Error(ProviderError::Rejected {
            status: Some(400),
            message: "`temperature` is deprecated\nfor this model.".to_owned(),
        }),
        Step::Error(ProviderError::Rejected {
            status: None,
            message: String::new(),
        }),
    ]);
    let mut session = Session::new(SynthOptions::default());
    let runner = LeafRunner::new();
    let mut diagnostics = Vec::new();
    for _ in 0..2 {
        let Outcome::Failed(failure) = block_on(synthesize(&mut session, &provider, &task(&program), &runner)) else {
            panic!("it fails")
        };
        assert_eq!(failure.diagnostic.code, Code::ProviderNotConfigured);
        diagnostics.push(failure.diagnostic);
    }
    assert_eq!(provider.calls(), 2);
    assert_eq!(
        diagnostics[0].message,
        "I can't write `Double` because the provider rejected the request (HTTP 400)."
    );
    assert_eq!(
        diagnostics[0].notes,
        ["The provider said: \"`temperature` is deprecated for this model.\""]
    );
    assert_eq!(
        diagnostics[1].message,
        "I can't write `Double` because the provider rejected the request."
    );
    assert!(diagnostics[1].notes.is_empty());
}

/// A rejected key is `VL0405` in its own words, and, like `VL0404`, ends all contact: the next goal gets the same
/// diagnostic with no request (R-SYNTH-45, R-SYNTH-07).
#[test]
fn r_synth_45_a_rejected_key_stops_further_contact_with_vl0405() {
    let program = program(SOURCE);
    let provider = Scripted::new([Step::Error(ProviderError::KeyRejected), Step::Reply("{}".to_owned())]);
    let mut session = Session::new(SynthOptions::default());
    let runner = LeafRunner::new();
    for _ in 0..2 {
        let Outcome::Failed(failure) = block_on(synthesize(&mut session, &provider, &task(&program), &runner)) else {
            panic!("it fails")
        };
        assert_eq!(failure.diagnostic.code, Code::ProviderNotConfigured);
        assert!(
            failure.diagnostic.message.contains("the API key was rejected"),
            "{}",
            failure.diagnostic.message
        );
    }
    assert_eq!(provider.calls(), 1);
}

/// A replay file that can't be written or read ends the goal with `VL0901` and, like `VL0404`, all contact with the
/// provider: the next goal gets the same diagnostic with no request (R-SYNTH-45).
#[test]
fn r_synth_45_a_file_error_stops_further_contact_with_vl0901() {
    let program = program(SOURCE);
    let file = || ProviderError::File {
        path: "fixtures/b3-x.json".to_owned(),
        reason: "it is not a regular file".to_owned(),
    };
    let provider = Scripted::new([Step::Error(file()), Step::Reply("{}".to_owned())]);
    let mut session = Session::new(SynthOptions::default());
    let runner = LeafRunner::new();
    for _ in 0..2 {
        let Outcome::Failed(failure) = block_on(synthesize(&mut session, &provider, &task(&program), &runner)) else {
            panic!("it fails")
        };
        assert_eq!(failure.diagnostic.code, Code::FileError);
    }
    assert_eq!(provider.calls(), 1);
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
    // The learner's line is the rule in words; the id is there for tools and bug reports (P-6).
    assert!(
        failed.attempts[0].contains("calls another goal itself") && failed.attempts[0].contains("(structure-6)"),
        "{:?}",
        failed.attempts
    );
    assert!(!d.message.contains("structure-6"), "{}", d.message);
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
    assert!(d.message.contains("the reply had no `body`"), "{}", d.message);
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
