//! The IR validator (`compiler/21` §6–7): the golden accept/reject corpus and the criteria of `compiler/21` it owns.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_builtins::limits::MAX_LIST_SIZE;
use velme_diagnostics::render::{JsonDiagnostic, LineIndex, render_human};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::limits::{MAX_COLLECTION_NESTING, MAX_DEPTH, MAX_IR_BYTES, MAX_LIST_ITEMS, MAX_NODES, MAX_TEXT_BYTES};
use velme_ir::{CallNode, Goal, IR_VERSION, MAX_JSON_DEPTH, Origin, Request, ValidIr, from_json_str, validate};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn program(text: &str) -> Program {
    let (program, diags) = analyze(&SourceFile::new("test.velme", text));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

/// The program of the golden IR corpus.
fn goals() -> Program {
    program(&read(&repo("tests/golden/ir/goals.velme")))
}

fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(
        program
            .goals
            .iter()
            .position(|g| g.name == name)
            .unwrap_or_else(|| panic!("no goal {name}")),
    )
}

/// The compiler's call section of a golden goal: the calls of its accept file, whose signatures are the children's
/// (`tests/fingerprint.rs`).
fn compiler_calls(goal: &str) -> Vec<CallNode> {
    if goal != "BuildPlayerSummary" {
        return Vec::new();
    }
    let golden: Goal = from_json_str(&read(&repo("tests/golden/ir/accept/player_summary.json"))).expect("parses");
    golden.calls
}

fn run(program: &Program, goal: &str, origin: Origin, text: &str) -> Result<ValidIr, Vec<Diagnostic>> {
    let calls = compiler_calls(goal);
    let request = Request {
        program,
        goal: goal_id(program, goal),
        calls: &calls,
        origin,
    };
    validate(text, &request)
}

fn accepts(program: &Program, goal: &str, text: &str) -> ValidIr {
    run(program, goal, Origin::Complete, text).unwrap_or_else(|d| panic!("{d:#?}"))
}

/// The one diagnostic `text` is rejected with; its code must be `code`.
fn rejects(program: &Program, goal: &str, origin: Origin, text: &str, code: Code) -> Diagnostic {
    let diags = run(program, goal, origin, text).expect_err("rejected");
    assert_eq!(diags.len(), 1, "{diags:#?}");
    let diag = diags.into_iter().next().expect("one diagnostic");
    assert_eq!(diag.code, code, "{diag:#?}");
    diag
}

fn invalid(program: &Program, goal: &str, text: &str) -> Diagnostic {
    rejects(program, goal, Origin::Complete, text, Code::IRInvalid)
}

/// `diag` points at `path` (R-IR-19).
fn at(diag: &Diagnostic, path: &str) {
    assert!(
        diag.notes.first().is_some_and(|n| n == &format!("at `{path}`")),
        "{diag:#?}"
    );
}

fn golden(name: &str) -> Value {
    from_json_str(&read(&repo(&format!("tests/golden/ir/accept/{name}.json")))).expect("golden parses")
}

/// `value` with the member at `pointer` replaced by `with`.
fn edit(mut value: Value, pointer: &str, with: Value) -> String {
    *value.pointer_mut(pointer).unwrap_or_else(|| panic!("no {pointer}")) = with;
    value.to_string()
}

/// A goal document for the test programs below.
fn document(goal: &str, inputs: Value, output: Value, body: &str) -> String {
    format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "{goal}", "types": {{}},
            "inputs": {inputs}, "output": {output}, "body": {body}}}"#
    )
}

const NUMBER: &str = r#"{"kind": "literal", "type": {"t": "Number"}, "value": 1}"#;

// ---- golden corpus ----

#[test]
fn golden_ir_validates() {
    let program = goals();
    insta::glob!("../../../tests/golden/ir/accept", "*.json", |path| {
        let text = read(path);
        let goal: Goal = from_json_str(&text).expect("golden parses");
        let valid = accepts(&program, &goal.goal, &text);
        assert_eq!(valid.goal(), &goal);
    });
}

#[test]
fn candidates_get_the_compiler_calls_joined_in() {
    // R-CMP-08: the reply is the tail only; the validated goal carries the compiler's call section.
    let program = goals();
    let mut candidate = golden("player_summary");
    candidate.as_object_mut().expect("object").remove("calls");
    let valid = run(
        &program,
        "BuildPlayerSummary",
        Origin::Candidate,
        &candidate.to_string(),
    )
    .expect("valid");
    assert_eq!(valid.into_goal().calls, compiler_calls("BuildPlayerSummary"));
}

/// Rejecting golden IR: each file is validated against the goal it names (or `FindBadge`), and its diagnostics are
/// snapshotted as rendered text and `--json` (R-QA-05).
#[test]
fn golden_ir_rejects() {
    let source = read(&repo("tests/golden/ir/goals.velme"));
    let program = goals();
    insta::glob!("../../../tests/golden/ir/reject", "*.json", |path| {
        let text = read(path);
        let named = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["goal"].as_str().map(str::to_owned))
            .filter(|g| program.goals.iter().any(|p| &p.name == g));
        let goal = named.as_deref().unwrap_or("FindBadge");
        let diags = run(&program, goal, Origin::Complete, &text).expect_err("rejected");
        let file = "tests/golden/ir/goals.velme";
        let lines = LineIndex::new(&source);
        let json: Vec<_> = diags.iter().map(|d| JsonDiagnostic::new(d, file, &lines)).collect();
        insta::assert_snapshot!(format!(
            "{}\n{}",
            render_human(&diags, file, Some(&source), false),
            serde_json::to_string_pretty(&json).expect("diagnostics serialize")
        ));
    });
}

// ---- compiler/21 criteria ----

#[test]
fn ac_ir_02_calls_from_synthesized_ir_are_rejected() {
    let program = goals();
    // A candidate that sends the call section itself.
    let with_calls = read(&repo("tests/golden/ir/accept/player_summary.json"));
    let diag = rejects(
        &program,
        "BuildPlayerSummary",
        Origin::Candidate,
        &with_calls,
        Code::IRInvalid,
    );
    at(&diag, "/calls");
    // A `call` node inside the body, from a candidate or in a complete goal (INV-6).
    let call = json!({"kind": "call", "binding": "b", "goal": "CalculateScore", "goal_signature": "b3:00",
                      "args": [{"kind": "input", "name": "player"}]});
    let text = edit(golden("find_badge"), "/body/then", call.clone());
    for origin in [Origin::Candidate, Origin::Complete] {
        let diag = rejects(&program, "FindBadge", origin, &text, Code::IRInvalid);
        at(&diag, "/body/then");
        assert!(diag.message.contains("`CalculateScore`"), "{diag:#?}");
    }
    let text = edit(golden("player_summary"), "/calls/0/args/0", call);
    at(&invalid(&program, "BuildPlayerSummary", &text), "/calls/0/args/0");
}

#[test]
fn ac_ir_03_body_type_must_be_the_output_type() {
    let program = goals();
    let text = edit(
        golden("find_badge"),
        "/body",
        serde_json::from_str(NUMBER).expect("JSON"),
    );
    let diag = invalid(&program, "FindBadge", &text);
    at(&diag, "/body");
    assert!(
        diag.message.contains("Number") && diag.message.contains("Text"),
        "{diag:#?}"
    );
}

#[test]
fn ac_ir_04_unknown_builtin_is_named() {
    let program = goals();
    let text = edit(golden("find_badge"), "/body/else/then/name", json!("sqrt"));
    let diag = invalid(&program, "FindBadge", &text);
    at(&diag, "/body/else/then/name");
    assert!(diag.message.contains("`sqrt`"), "{diag:#?}");
    // A collection primitive is a node of its own, not a `builtin` call.
    let text = edit(golden("find_badge"), "/body/else/then/name", json!("map"));
    invalid(&program, "FindBadge", &text);
}

#[test]
fn literal_over_the_item_limit_still_reports_a_wrong_item() {
    let program = program(LIMITS);
    // Above both the §7 literal limit and `max_list_size`, which the D-23 decoder checks after the items.
    let mut items = vec![json!(0); usize::try_from(MAX_LIST_SIZE).expect("small") + 1];
    items[0] = json!("x");
    let body = json!({"kind": "literal", "type": {"t": "List", "of": {"t": "Number"}}, "value": items});
    let text = limit_doc("Nested", json!({"t": "List", "of": {"t": "Number"}}), &body.to_string());
    let diags = run(&program, "Nested", Origin::Complete, &text).expect_err("rejected");
    let notes: Vec<&String> = diags.iter().flat_map(|d| &d.notes).collect();
    assert!(notes.iter().any(|n| *n == "at `/body/value/0`"), "{diags:#?}");
    assert!(diags.iter().any(|d| d.message.contains("isn't Number")), "{diags:#?}");
}

#[test]
fn ac_blt_08_ir_calling_an_unknown_builtin_is_ir_invalid() {
    let program = goals();
    let text = edit(golden("find_badge"), "/body/else/then/name", json!("median"));
    assert_eq!(invalid(&program, "FindBadge", &text).code, Code::IRInvalid);
}

const LIMITS: &str = "\
goal Nodes(xs: List<Number>) -> List<List<Number>>:\n    plan: \"Test.\"\n\n\
goal Deep(xs: List<Number>) -> List<Number?>:\n    plan: \"Test.\"\n\n\
goal Nested(xs: List<Number>) -> List<Number>:\n    plan: \"Test.\"\n\n\
goal Words(xs: List<Number>) -> Text:\n    plan: \"Test.\"\n";

fn limit_doc(goal: &str, output: Value, body: &str) -> String {
    document(
        goal,
        json!([["xs", {"t": "List", "of": {"t": "Number"}}]]),
        output,
        body,
    )
}

/// Accepted at `at`; one past it is `VL0402` naming the limit.
fn limit(program: &Program, goal: &str, doc: impl Fn(usize) -> String, at: usize) {
    accepts(program, goal, &doc(at));
    let diag = invalid(program, goal, &doc(at + 1));
    assert!(diag.message.contains(&at.to_string()), "{diag:#?}");
}

#[test]
fn ac_ir_05_each_limit_is_inclusive() {
    let program = program(LIMITS);
    let numbers = json!({"t": "List", "of": {"t": "Number"}});
    // Nodes: an outer list of ten lists of literals, `n` nodes in all.
    let nodes = |n: usize| {
        let inner = |len: usize| {
            format!(
                r#"{{"kind": "list", "of": {{"t": "Number"}}, "items": [{}]}}"#,
                vec![NUMBER; len].join(",")
            )
        };
        let mut left = n - 11;
        let lists: Vec<String> = (0..10)
            .map(|_| {
                let len = left.min(MAX_LIST_ITEMS - 1);
                left -= len;
                inner(len)
            })
            .collect();
        assert_eq!(left, 0);
        let body = format!(r#"{{"kind": "list", "of": {numbers}, "items": [{}]}}"#, lists.join(","));
        limit_doc("Nodes", json!({"t": "List", "of": numbers}), &body)
    };
    limit(&program, "Nodes", nodes, MAX_NODES);
    // Depth: `let`s around a literal, the shape that nests JSON deepest per expression.
    let deep = |depth: usize| {
        let mut body =
            r#"{"kind": "literal", "type": {"t": "List", "of": {"t": "Optional", "of": {"t": "Number"}}}, "value": [1]}"#
                .to_owned();
        for _ in 1..depth {
            body = format!(r#"{{"kind": "let", "bind": [["x", {body}]], "body": {{"kind": "local", "name": "x"}}}}"#);
        }
        limit_doc(
            "Deep",
            json!({"t": "List", "of": {"t": "Optional", "of": {"t": "Number"}}}),
            &body,
        )
    };
    limit(&program, "Deep", deep, MAX_DEPTH);
    // Collection nesting (D-79): a `filter` whose lambda holds `all`s, each inside the last one's lambda.
    let nested = |n: usize| {
        let mut body = format!(
            r#"{{"kind": "binary", "op": "gt", "left": {{"kind": "local", "name": "p{}"}}, "right": {NUMBER}}}"#,
            n - 1
        );
        for i in (0..n).rev() {
            let kind = if i == 0 { "filter" } else { "all" };
            body = format!(
                r#"{{"kind": "{kind}", "list": {{"kind": "input", "name": "xs"}}, "fn": {{"param": "p{i}", "body": {body}}}}}"#
            );
        }
        limit_doc("Nested", numbers.clone(), &body)
    };
    limit(&program, "Nested", nested, MAX_COLLECTION_NESTING);
    // Items of a `list` node, and elements of a literal's array.
    let items = |n: usize| {
        let body = format!(
            r#"{{"kind": "list", "of": {{"t": "Number"}}, "items": [{}]}}"#,
            vec![NUMBER; n].join(",")
        );
        limit_doc("Nested", numbers.clone(), &body)
    };
    limit(&program, "Nested", items, MAX_LIST_ITEMS);
    let array = |n: usize| {
        let body = format!(
            r#"{{"kind": "literal", "type": {numbers}, "value": [{}]}}"#,
            vec!["2"; n].join(",")
        );
        limit_doc("Nested", numbers.clone(), &body)
    };
    limit(&program, "Nested", array, MAX_LIST_ITEMS);
    // Text literal bytes: two-byte characters, so bytes, not characters, are counted.
    let words = |bytes: usize| {
        let text = "é".repeat(bytes / 2) + &"a".repeat(bytes % 2);
        let body = format!(r#"{{"kind": "literal", "type": {{"t": "Text"}}, "value": "{text}"}}"#);
        limit_doc("Words", json!({"t": "Text"}), &body)
    };
    limit(&program, "Words", words, MAX_TEXT_BYTES);
    // Document size: a small goal padded with spaces.
    let small = items(1);
    let sized = |bytes: usize| format!("{small}{}", " ".repeat(bytes - small.len()));
    limit(&program, "Nested", sized, MAX_IR_BYTES);
}

/// AC-IR-06: the validator is total on arbitrary JSON and on mutations of valid IR (R-IR-18). The fuzz target is
/// `fuzz/fuzz_targets/validate.rs`; its corpus is replayed here on stable.
#[test]
fn ac_ir_06_fuzz_corpus_replays_without_panic() {
    let program = goals();
    let mut count = 0;
    for dir in [
        "fuzz/corpus/validate",
        "tests/golden/ir/accept",
        "tests/golden/ir/reject",
    ] {
        for entry in std::fs::read_dir(repo(dir)).expect("corpus dir") {
            let bytes = std::fs::read(entry.expect("dir entry").path()).expect("corpus file");
            let text = String::from_utf8_lossy(&bytes);
            for goal in &program.goals {
                let _ = run(&program, &goal.name, Origin::Complete, &text);
                let _ = run(&program, &goal.name, Origin::Candidate, &text);
            }
            count += 1;
        }
    }
    assert!(count > 20, "{count}");
}

/// The deepest IR the JSON guard lets through validates without exhausting a test thread's stack (R-IR-18).
#[test]
fn deepest_parsable_ir_is_rejected_without_overflow() {
    let program = program(LIMITS);
    let depth = MAX_JSON_DEPTH - 3;
    let chain = r#"{"kind": "unary", "op": "not", "arg": "#.repeat(depth)
        + r#"{"kind": "input", "name": "xs"}"#
        + &"}".repeat(depth);
    let diags = run(
        &program,
        "Words",
        Origin::Complete,
        &limit_doc("Words", json!({"t": "Text"}), &chain),
    )
    .expect_err("too deep");
    // Stage 4 (a `not` of a list) comes before stage 7 (depth).
    assert!(
        diags
            .iter()
            .all(|d| d.code == Code::IRInvalid && !d.message.contains("deep")),
        "{diags:#?}"
    );
}

#[test]
fn ac_ir_09_newer_ir_version_names_both_versions() {
    let program = goals();
    let diag = invalid(
        &program,
        "FindBadge",
        &edit(golden("find_badge"), "/ir_version", json!("0.2")),
    );
    at(&diag, "/ir_version");
    assert!(diag.message.contains("0.2") && diag.message.contains(IR_VERSION) && diag.message.contains("newer"));
    let diag = invalid(
        &program,
        "FindBadge",
        &edit(golden("find_badge"), "/ir_version", json!("1.0")),
    );
    assert!(
        diag.message.contains("1.0") && diag.message.contains(IR_VERSION),
        "{diag:#?}"
    );
}

const COLLECTIONS: &str = "\
type Player:\n    name: Text\n    score: Number\n\n\
goal Check(xs: List<Number>) -> Boolean:\n    plan: \"Test.\"\n\n\
goal Rank(players: List<Player>) -> List<Player>:\n    plan: \"Test.\"\n\n\
goal Empty(xs: List<Number>?, t: Text?) -> Boolean:\n    plan: \"Test.\"\n\n\
goal Same(a: Number, b: Number?) -> Boolean:\n    plan: \"Test.\"\n";

fn check_doc(body: &str) -> String {
    document(
        "Check",
        json!([["xs", {"t": "List", "of": {"t": "Number"}}]]),
        json!({"t": "Boolean"}),
        body,
    )
}

#[test]
fn ac_ir_10_all_and_any_need_boolean_bodies() {
    let program = program(COLLECTIONS);
    let empty = r#"{"kind": "literal", "type": {"t": "List", "of": {"t": "Number"}}, "value": []}"#;
    let positive = r#"{"kind": "binary", "op": "gt", "left": {"kind": "local", "name": "p"}, "right": {"kind": "literal", "type": {"t": "Number"}, "value": 0}}"#;
    for kind in ["all", "any"] {
        let node = |list: &str, body: &str| {
            format!(r#"{{"kind": "{kind}", "list": {list}, "fn": {{"param": "p", "body": {body}}}}}"#)
        };
        accepts(&program, "Check", &check_doc(&node(empty, positive)));
        let diag = invalid(
            &program,
            "Check",
            &check_doc(&node(
                r#"{"kind": "input", "name": "xs"}"#,
                r#"{"kind": "local", "name": "p"}"#,
            )),
        );
        at(&diag, "/body/fn/body");
        assert!(diag.message.contains("Boolean"), "{diag:#?}");
    }
}

#[test]
fn ac_ir_11_sort_by_key_is_a_number() {
    let program = program(COLLECTIONS);
    let sort = |field: &str| {
        let body = format!(
            r#"{{"kind": "sort_by", "list": {{"kind": "input", "name": "players"}}, "descending": false,
                "key": {{"param": "p", "body": {{"kind": "field", "of": {{"kind": "local", "name": "p"}}, "field": "{field}"}}}}}}"#
        );
        let player = json!({"t": "Record", "name": "Player"});
        format!(
            r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Rank",
                "types": {{"Player": {{"fields": [["name", {{"t": "Text"}}], ["score", {{"t": "Number"}}]]}}}},
                "inputs": [["players", {{"t": "List", "of": {player}}}]], "output": {{"t": "List", "of": {player}}},
                "body": {body}}}"#
        )
    };
    accepts(&program, "Rank", &sort("score"));
    let diag = invalid(&program, "Rank", &sort("name"));
    at(&diag, "/body/key/body");
    assert!(diag.message.contains("text"), "{diag:#?}");
}

#[test]
fn ac_ir_12_is_empty_on_optional_collections() {
    let program = program(COLLECTIONS);
    let body = r#"{"kind": "binary", "op": "and",
        "left": {"kind": "unary", "op": "is_empty", "arg": {"kind": "input", "name": "xs"}},
        "right": {"kind": "unary", "op": "is_empty", "arg": {"kind": "input", "name": "t"}}}"#;
    let inputs = json!([["xs", {"t": "Optional", "of": {"t": "List", "of": {"t": "Number"}}}],
                        ["t", {"t": "Optional", "of": {"t": "Text"}}]]);
    accepts(
        &program,
        "Empty",
        &document("Empty", inputs.clone(), json!({"t": "Boolean"}), body),
    );
    // `is_empty` on a Number is not.
    let number =
        r#"{"kind": "unary", "op": "is_empty", "arg": {"kind": "literal", "type": {"t": "Number"}, "value": 1}}"#;
    invalid(
        &program,
        "Empty",
        &document("Empty", inputs, json!({"t": "Boolean"}), number),
    );
}

#[test]
fn ac_ir_13_equality_needs_assignability_one_way() {
    let program = program(COLLECTIONS);
    let inputs = json!([["a", {"t": "Number"}], ["b", {"t": "Optional", "of": {"t": "Number"}}]]);
    let eq = |op: &str, right: &str| {
        let body = format!(
            r#"{{"kind": "binary", "op": "{op}", "left": {{"kind": "input", "name": "a"}}, "right": {right}}}"#
        );
        document("Same", inputs.clone(), json!({"t": "Boolean"}), &body)
    };
    accepts(&program, "Same", &eq("eq", r#"{"kind": "input", "name": "b"}"#));
    accepts(&program, "Same", &eq("ne", r#"{"kind": "input", "name": "b"}"#));
    let text = r#"{"kind": "literal", "type": {"t": "Text"}, "value": "1"}"#;
    let diag = invalid(&program, "Same", &eq("eq", text));
    assert!(
        diag.message.contains("Number") && diag.message.contains("Text"),
        "{diag:#?}"
    );
}

#[test]
fn ac_ir_14_versions_must_equal_the_request() {
    let program = goals();
    for (pointer, own) in [("/ir_version", IR_VERSION), ("/builtins_version", BUILTINS_VERSION)] {
        let older = edit(golden("find_badge"), pointer, json!("0.0"));
        let diag = rejects(&program, "FindBadge", Origin::Candidate, &older, Code::IRInvalid);
        at(&diag, pointer);
        assert!(diag.message.contains("0.0") && diag.message.contains(own), "{diag:#?}");
        // R-IR-22: a complete goal, such as a stored artifact, of an older minor of the same major is still read.
        accepts(&program, "FindBadge", &older);
        let diag = invalid(
            &program,
            "FindBadge",
            &edit(golden("find_badge"), pointer, json!("1.0")),
        );
        at(&diag, pointer);
    }
}

// ---- other validator rules ----

#[test]
fn schema_failures_are_vl0401() {
    // R-IR-11; an unknown node kind or field fails here, so "kinds known" (R-IR-12) never reaches stage 2 (D-63).
    let program = goals();
    for (text, note) in [
        ("{".to_owned(), None),
        (edit(golden("find_badge"), "/body/kind", json!("loop")), None),
        (
            {
                let mut ir = golden("find_badge");
                ir["body"]["host"] = json!("fs");
                ir.to_string()
            },
            None,
        ),
        (
            read(&repo("tests/golden/ir/accept/find_badge.json")).replacen(
                r#""goal": "#,
                r#""goal": "X", "goal": "#,
                1,
            ),
            Some("at `/goal`: this key appears twice"),
        ),
    ] {
        let diag = rejects(&program, "FindBadge", Origin::Complete, &text, Code::IRSchemaInvalid);
        if let Some(note) = note {
            assert_eq!(diag.notes, [note]);
        }
    }
}

#[test]
fn earliest_failing_stage_is_the_one_reported() {
    let program = program(LIMITS);
    // An unknown name (stage 3) and a type error (stage 4) inside a too-deep body (stage 7): only stage 3.
    let mut body = r#"{"kind": "binary", "op": "add", "left": {"kind": "local", "name": "nope"}, "right": {"kind": "literal", "type": {"t": "Text"}, "value": ""}}"#.to_owned();
    for _ in 0..MAX_DEPTH {
        body = format!(r#"{{"kind": "unary", "op": "neg", "arg": {body}}}"#);
    }
    let body = format!(r#"{{"kind": "builtin", "name": "to_text", "args": [{body}]}}"#);
    let diag = invalid(&program, "Words", &limit_doc("Words", json!({"t": "Text"}), &body));
    assert!(diag.message.contains("`nope`"), "{diag:#?}");
    // Every finding of the failing stage is reported.
    let two = r#"{"kind": "builtin", "name": "concat", "args": [{"kind": "local", "name": "a"}, {"kind": "local", "name": "b"}]}"#;
    let diags = run(
        &program,
        "Words",
        Origin::Complete,
        &limit_doc("Words", json!({"t": "Text"}), two),
    )
    .expect_err("rejected");
    assert_eq!(diags.len(), 2, "{diags:#?}");
}

#[test]
fn goal_signature_and_types_must_match_the_program() {
    let program = goals();
    let diag = invalid(
        &program,
        "FindBadge",
        &edit(golden("find_badge"), "/goal", json!("FindMedal")),
    );
    at(&diag, "/goal");
    let diag = invalid(
        &program,
        "FindBadge",
        &edit(golden("find_badge"), "/types/Player/fields/2/1", json!({"t": "Text"})),
    );
    at(&diag, "/types/Player");
    let mut extra = golden("find_badge");
    extra["inputs"]
        .as_array_mut()
        .expect("inputs")
        .push(json!(["bonus", {"t": "Number"}]));
    let diag = invalid(&program, "FindBadge", &extra.to_string());
    at(&diag, "/inputs");
    let diag = invalid(
        &program,
        "FindBadge",
        &edit(golden("find_badge"), "/inputs/0/1/name", json!("Players")),
    );
    at(&diag, "/inputs/0/1/name");
    assert_eq!(diag.help.as_deref(), Some("did you mean `Player`?"));
}

#[test]
fn names_resolve_and_never_shadow() {
    let program = goals();
    let jumper = golden("find_highest_jumper");
    let diag = invalid(
        &program,
        "FindHighestJumpingPlayer",
        &edit(jumper.clone(), "/body/bind/0/0", json!("p")),
    );
    // `p` is bound by the `let` and again by the lambda of its body (R-IR-12).
    at(&diag, "/body/body/fn/param");
    let diag = invalid(
        &program,
        "FindHighestJumpingPlayer",
        &edit(jumper.clone(), "/body/body/fn/body/right/of/name", json!("bets")),
    );
    assert_eq!(diag.help.as_deref(), Some("did you mean `best`?"));
    let diag = invalid(
        &program,
        "FindHighestJumpingPlayer",
        &edit(jumper, "/body/body/fn/body/left/field", json!("height")),
    );
    at(&diag, "/body/body/fn/body/left/field");
    let team = golden("team_stats");
    let diag = invalid(
        &program,
        "SummarizeTeam",
        &edit(team, "/body/bind/1/1/fn/acc", json!("p")),
    );
    at(&diag, "/body/bind/1/1/fn/param");
}

#[test]
fn optionals_are_narrowed_explicitly() {
    // R-IR-05: `field` on a `Player?` is invalid; `find` gives one.
    let program = goals();
    let top = json!({"kind": "field", "field": "score", "of": golden("team_stats")["body"]["body"]["fields"]["top"]});
    let text = edit(golden("team_stats"), "/body/body/fields/total", top);
    let diag = invalid(&program, "SummarizeTeam", &text);
    at(&diag, "/body/body/fields/total");
    assert!(diag.message.contains("Player?"), "{diag:#?}");
}

#[test]
fn literals_decode_as_their_type() {
    let program = goals();
    for (value, path) in [
        (json!("100"), "/body/cond/right/value"),
        (serde_json::from_str("1e40").expect("JSON"), "/body/cond/right/value"),
    ] {
        at(
            &invalid(
                &program,
                "FindBadge",
                &edit(golden("find_badge"), "/body/cond/right/value", value),
            ),
            path,
        );
    }
    let player = json!({"kind": "literal", "type": {"t": "Record", "name": "Player"},
                        "value": {"name": "Ana", "score": 3}});
    let text = edit(golden("find_badge"), "/body/cond/left/of", player);
    let diag = invalid(&program, "FindBadge", &text);
    assert!(diag.message.contains("`jump_height`"), "{diag:#?}");
}

#[test]
fn calls_must_equal_the_compiler_calls() {
    // R-IR-16, stage 6.
    let program = goals();
    let summary = golden("player_summary");
    let diag = invalid(
        &program,
        "BuildPlayerSummary",
        &edit(summary.clone(), "/calls/1/goal_signature", json!("b3:ff")),
    );
    at(&diag, "/calls/1");
    // A call the `call` block doesn't have, even one no expression uses.
    let mut extra = summary;
    let mut call = extra["calls"][0].clone();
    call["binding"] = json!("again");
    extra["calls"].as_array_mut().expect("calls").push(call);
    let diag = invalid(&program, "BuildPlayerSummary", &extra.to_string());
    at(&diag, "/calls/3");
    assert!(diag.message.contains("`again`"), "{diag:#?}");
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1000i64..1000).prop_map(Value::from),
        "[a-z_]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 24, 5, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z_]{1,6}", inner, 0..5)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Every JSON Pointer inside `value`.
fn pointers(value: &Value, at: String, out: &mut Vec<String>) {
    match value {
        Value::Object(members) => members.iter().for_each(|(k, v)| pointers(v, format!("{at}/{k}"), out)),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .for_each(|(i, v)| pointers(v, format!("{at}/{i}"), out)),
        _ => {}
    }
    out.push(at);
}

proptest! {
    /// AC-IR-06: arbitrary JSON, and golden IR with one member replaced by arbitrary JSON or by another part of the
    /// same file (which keeps most mutations schema-valid), never make the validator panic.
    #[test]
    fn ac_ir_06_validator_is_total(value in json_value(), pick in any::<prop::sample::Index>(),
                                   from in any::<prop::sample::Index>(), swap in any::<bool>()) {
        let program = goals();
        let _ = run(&program, "FindBadge", Origin::Complete, &value.to_string());
        for name in ["find_badge", "player_summary", "team_stats", "find_highest_jumper"] {
            let ir = golden(name);
            let mut all = Vec::new();
            pointers(&ir, String::new(), &mut all);
            let target = pick.get(&all).clone();
            let with = if swap { ir.pointer(from.get(&all)).cloned().unwrap_or(Value::Null) } else { value.clone() };
            let text = if target.is_empty() { with.to_string() } else { edit(ir.clone(), &target, with) };
            let goal = ir["goal"].as_str().unwrap_or_default().to_owned();
            let _ = run(&program, &goal, Origin::Complete, &text);
            let _ = run(&program, &goal, Origin::Candidate, &text);
        }
    }
}

#[test]
fn candidate_calls_resolve_against_the_goal_signature() {
    // The joined-in calls use the goal's own input names, so a candidate that renames an input is told about its
    // inputs, not about calls it never wrote.
    let program = goals();
    let mut candidate = golden("player_summary");
    candidate.as_object_mut().expect("object").remove("calls");
    candidate["inputs"][0][0] = json!("p");
    let diag = rejects(
        &program,
        "BuildPlayerSummary",
        Origin::Candidate,
        &candidate.to_string(),
        Code::IRInvalid,
    );
    at(&diag, "/inputs");
}

#[test]
fn nested_optional_types_are_one_optional() {
    // §2.1: `Number??` is `Number?`.
    let program = program("goal Maybe(xs: List<Number>) -> Number?:\n    plan: \"Test.\"\n");
    let twice = json!({"t": "Optional", "of": {"t": "Optional", "of": {"t": "Number"}}});
    let body = format!(r#"{{"kind": "literal", "type": {twice}, "value": null}}"#);
    accepts(&program, "Maybe", &limit_doc("Maybe", twice, &body));
}

#[test]
fn types_list_the_record_types_of_called_goals() {
    // R-IR-01: reachable from `calls` too.
    let program = program(
        "type Player:\n    name: Text\n\ntype Badge:\n    label: Text\n\n\
         goal Award(player: Player) -> Badge:\n    plan: \"Test.\"\n\n\
         goal Show(player: Player) -> Text:\n    call:\n        badge = Award(player)\n    plan: \"Test.\"\n",
    );
    let calls: Vec<CallNode> = serde_json::from_value(json!([{"kind": "call", "binding": "badge", "goal": "Award",
        "goal_signature": "b3:00", "args": [{"kind": "input", "name": "player"}]}]))
    .expect("calls");
    let doc = |types: Value| {
        json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "Show", "types": types,
               "inputs": [["player", {"t": "Record", "name": "Player"}]], "output": {"t": "Text"}, "calls": calls,
               "body": {"kind": "field", "of": {"kind": "local", "name": "badge"}, "field": "label"}})
        .to_string()
    };
    let request = Request {
        program: &program,
        goal: goal_id(&program, "Show"),
        calls: &calls,
        origin: Origin::Complete,
    };
    let player = json!({"fields": [["name", {"t": "Text"}]]});
    let badge = json!({"fields": [["label", {"t": "Text"}]]});
    validate(&doc(json!({"Player": player, "Badge": badge})), &request).expect("valid");
    let diags = validate(&doc(json!({"Player": player})), &request).expect_err("rejected");
    assert_eq!(diags.len(), 1, "{diags:#?}");
    at(&diags[0], "/types");
    assert!(diags[0].message.contains("`Badge`"), "{diags:#?}");
}

#[test]
fn oversized_documents_are_rejected_before_parsing() {
    let program = goals();
    let text = format!("{{{}", " ".repeat(MAX_IR_BYTES));
    let diag = invalid(&program, "FindBadge", &text);
    assert!(diag.message.contains(&MAX_IR_BYTES.to_string()), "{diag:#?}");
}

#[test]
fn flat_pipelines_do_not_nest() {
    // D-79: ops in `list` position run once each, so a long pipeline is within the collection-nesting limit.
    let program = program(LIMITS);
    let mut body = r#"{"kind": "input", "name": "xs"}"#.to_owned();
    for _ in 0..(MAX_COLLECTION_NESTING + 3) {
        body = format!(
            r#"{{"kind": "map", "list": {body}, "fn": {{"param": "p", "body": {{"kind": "local", "name": "p"}}}}}}"#
        );
    }
    accepts(
        &program,
        "Nested",
        &limit_doc("Nested", json!({"t": "List", "of": {"t": "Number"}}), &body),
    );
}
