//! Reply notation and its places (`compiler/22` §4.2, D-157).
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use serde_json::{Value, json};
use velme_ir::{Node, Origin, Request, calls, to_canonical_string, validate_detailed};
use velme_synth::{
    Outcome, ReplyFormat, Scripted, Session, Step, SynthOptions, Task, build_request, notation, synthesize,
};
use velme_test_support::{goal_id, program, read, repo};

const SOURCE: &str = include_str!("notation/goals.velme");

fn lowered(text: &str, goal: &str) -> Result<notation::Lowered, notation::Rejected> {
    let program = program(SOURCE);
    let id = goal_id(&program, goal);
    let request = build_request(&program, id, SOURCE).expect("request");
    notation::lower(
        text,
        &request,
        &Request {
            program: &program,
            goal: id,
            calls: &[],
            origin: Origin::Candidate,
        },
    )
}

fn body(text: &str, goal: &str) -> Node {
    lowered(text, goal).unwrap_or_else(|e| panic!("{text}: {e:?}")).body
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn run(goal: &str, replies: Vec<Value>) -> (Outcome, Scripted, Session) {
    let program = program(SOURCE);
    let id = goal_id(&program, goal);
    let task = Task {
        program: &program,
        goal: id,
        source: SOURCE,
        contract_key: velme_ir::contract_key(&program, id).expect("contract"),
        synthesis_key: velme_ir::Fingerprint::of_bytes(b"notation-test"),
        feedback: Vec::new(),
    };
    let provider = Scripted::new(replies.iter().map(|v| Step::Reply(v.to_string())));
    let mut session = Session::new(SynthOptions {
        reply_format: ReplyFormat::Notation,
        max_retries: replies.len().saturating_sub(1) as u32,
        stop_on_repeat: false,
        ..SynthOptions::default()
    });
    let outcome = block_on(synthesize(
        &mut session,
        &provider,
        &task,
        &velme_test_support::LeafRunner::new(),
    ));
    (outcome, provider, session)
}

#[test]
fn ac_synth_50_loan_bodies_lower_to_the_committed_ir_and_validate() {
    let dir = repo("crates/velme-synth/tests/notation/loan");
    let source = read(&dir.join("loan.velme"));
    let program = program(&source);
    let mut count = 0;
    for (i, goal) in program.goals.iter().enumerate() {
        let id = velme_sema::hir::GoalId(i);
        let request = build_request(&program, id, &source).expect("request");
        let calls = calls(&program, id).expect("calls");
        let ir = Request {
            program: &program,
            goal: id,
            calls: &calls,
            origin: Origin::Candidate,
        };
        let text = read(&dir.join(format!("{}.notation", goal.name)));
        let lowered = notation::lower(&text, &request, &ir).unwrap_or_else(|e| panic!("{}: {e:?}", goal.name));
        let expected: Value =
            velme_ir::from_json_str(&read(&dir.join(format!("{}.json", goal.name)))).expect("golden IR");
        let actual = serde_json::to_value(&lowered.body).expect("body JSON");
        assert_eq!(
            actual, expected["body"],
            "{}: changed lowering requires GRAMMAR_VERSION bumped",
            goal.name
        );
        let assembled = request.assemble(&actual).expect("assembly");
        assert!(validate_detailed(&assembled, &ir).is_ok(), "{}", goal.name);
        count += 1;
    }
    assert_eq!(count, 10);
}

#[test]
fn ac_synth_58_accepted_golden_bodies_match_their_ir() {
    let dir = repo("crates/velme-synth/tests/notation/accept");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("goldens")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "notation"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty());
    for path in entries {
        let text = read(&path);
        let expected: Value = velme_ir::from_json_str(&read(&path.with_extension("json"))).expect("expected");
        let goal = expected["goal"].as_str().expect("goal");
        let actual = body(&text, goal);
        let program = program(SOURCE);
        let id = goal_id(&program, goal);
        let request = build_request(&program, id, SOURCE).expect("request");
        let assembled = request
            .assemble(&serde_json::to_value(&actual).expect("body"))
            .expect("assembly");
        let ir = Request {
            program: &program,
            goal: id,
            calls: &[],
            origin: Origin::Candidate,
        };
        match (validate_detailed(&assembled, &ir), expected["reject_rule"].as_str()) {
            (Ok(_), None) => {}
            (Err(found), Some(rule)) => assert_eq!(found[0].rule, rule, "{}", path.display()),
            (found, _) => panic!("{}: {found:?}", path.display()),
        }
        assert_eq!(
            serde_json::to_value(&actual).expect("JSON"),
            expected["body"],
            "{}: changed lowering requires GRAMMAR_VERSION bumped",
            path.display()
        );
        let again = body(&text, goal);
        assert_eq!(
            to_canonical_string(&serde_json::to_value(actual).expect("JSON")).expect("canonical"),
            to_canonical_string(&serde_json::to_value(again).expect("JSON")).expect("canonical")
        );
    }
}

#[test]
fn ac_synth_51_rejected_goldens_have_the_rule_and_place() {
    let dir = repo("crates/velme-synth/tests/notation/reject");
    let expected: Value = velme_ir::from_json_str(&read(&dir.join("expected.json"))).expect("expectations");
    for (name, expected) in expected.as_object().expect("cases") {
        let text = read(&dir.join(format!("{name}.notation")));
        let rejected = lowered(&text, expected["goal"].as_str().unwrap_or("N")).expect_err(name);
        assert_eq!(rejected.rule.id(), expected["rule"], "{name}: {rejected:?}");
        let (line, column) = notation::locate(&text, rejected.span.start);
        assert_eq!(json!([line, column]), expected["at"], "{name}: {rejected:?}");
        if let Some(hint) = expected["hint"].as_str() {
            assert!(
                rejected.hint.as_deref().unwrap_or_default().contains(hint),
                "{name}: {rejected:?}"
            );
        }
        let (_, provider, mut session) = run(
            expected["goal"].as_str().unwrap_or("N"),
            vec![json!({"body":text}), json!({"body":"return 1"})],
        );
        let requests = provider.requests();
        let diagnostic = &requests[1].attempts[0].diagnostics[0];
        assert_eq!(
            diagnostic.code,
            velme_diagnostics::Code::IRSchemaInvalid.as_str(),
            "{name}"
        );
        assert!(diagnostic.message.contains(rejected.rule.id()));
        let place = diagnostic.at.as_ref().expect("place");
        assert_eq!(json!([place.line, place.column]), expected["at"], "{name}");
        assert_eq!(
            session.take_records()[0].cause.expect("cause").token(),
            rejected.rule.id()
        );
    }
}

#[test]
fn ac_synth_52_lists_take_only_the_specified_contexts() {
    for (text, goal) in [
        ("return []", "L"),
        ("return [] if c else xs", "L"),
        ("return xs == []", "L"),
        ("return Box(values=[])", "Boxed"),
        ("return sum([])", "N"),
        ("return unwrap_or(opt, [])", "L"),
        ("return reduce(lambda acc, i: acc, xs, [])", "L"),
        ("return [[], [1]]", "Nested"),
        ("a = [] if c else [1]\nreturn 1", "N"),
    ] {
        let lowered = lowered(text, goal).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        for node in lowered.body.preorder() {
            if let Node::List { of, .. } = node {
                assert_ne!(*of, velme_ir::Type::Nothing {}, "{text}");
            }
        }
    }
    for text in ["a = []\nreturn 1", "return len([])", "return contains([], 1)"] {
        assert_eq!(lowered(text, "N").expect_err(text).rule, notation::Rule::ListType);
    }
    let lowered = lowered("a = [unknown]\nreturn 1", "N").expect("the validator owns unknown names");
    let program = program(SOURCE);
    let id = goal_id(&program, "N");
    let request = build_request(&program, id, SOURCE).expect("request");
    let text = request
        .assemble(&serde_json::to_value(lowered.body).expect("body"))
        .expect("assembly");
    let found = validate_detailed(
        &text,
        &Request {
            program: &program,
            goal: id,
            calls: &[],
            origin: Origin::Candidate,
        },
    )
    .expect_err("unknown");
    assert_eq!(found[0].rule, "names-8");
}

#[test]
fn ac_synth_53_caps_and_determinism_on_a_two_mib_debug_stack() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            for text in [
                format!("return {}1{}", "(".repeat(129), ")".repeat(129)),
                format!("{}return 1", "#".repeat(65_537)),
                format!("if c: return 1\n{}else: return 0", "elif c: return 1\n".repeat(127)),
                format!("return {}x", "-".repeat(128)),
                format!("return {}True", "not ".repeat(128)),
                // Chains held in chains of another precedence count toward the nesting (R-SYNTH-57).
                format!("return {}x{}", "c or c and x == x + x * (".repeat(70), ")".repeat(70)),
            ] {
                let rejected = lowered(&text, "N").expect_err("cap");
                assert_eq!(rejected.rule, notation::Rule::Limit);
                assert_eq!(notation::locate(&text, rejected.span.start), (1, 1));
            }
            let chains = format!("return {}x{}", "c or c and x == x + x * (".repeat(70), ")".repeat(70));
            assert_eq!(lowered(&chains, "B").expect_err("cap").rule, notation::Rule::Limit);
            for text in [
                format!("return {}1{}", "(".repeat(128), ")".repeat(128)),
                format!("return {}x", "-".repeat(127)),
                format!("return {}True", "not ".repeat(127)),
                format!("return {}x{}", "c or c and x == x + x * (".repeat(25), ")".repeat(25)),
            ] {
                let one = body(&text, "N");
                let two = body(&text, "N");
                assert_eq!(one, two);
            }
        })
        .expect("thread")
        .join()
        .expect("no panic");
}

#[test]
fn ac_synth_61_caps_are_exact_and_lowered_chains_are_bounded() {
    let exact = format!("#{}\nreturn 1", "a".repeat(65_536 - 10));
    assert_eq!(exact.len(), 65_536);
    assert!(lowered(&exact, "N").is_ok());
    assert_eq!(
        lowered(&(exact + " "), "N").expect_err("size").rule,
        notation::Rule::Limit
    );
    for (items, accepted) in [(9_999, true), (10_000, false)] {
        let text = format!("return [{}]", vec!["1"; items].join(","));
        let result = lowered(&text, "L");
        assert_eq!(result.is_ok(), accepted);
        if let Err(rejected) = result {
            assert_eq!(rejected.rule, notation::Rule::Limit);
        }
    }
    let mut expr = "1".to_owned();
    for _ in 0..11 {
        expr = format!("0 < ({expr}) < 2");
    }
    assert_eq!(
        lowered(&format!("return {expr}"), "N")
            .expect_err("copies exceed node cap")
            .rule,
        notation::Rule::Limit
    );
    for (branches, accepted) in [(127, true), (128, false)] {
        let text = format!(
            "if c: return 1\n{}else: return 0",
            "elif c: return 1\n".repeat(branches - 1)
        );
        let result = lowered(&text, "N");
        assert_eq!(result.is_ok(), accepted, "{branches}");
    }
}

#[test]
fn ac_synth_53_validator_feedback_has_notation_places_and_hints() {
    for (goal, text, rule, hint, place) in [
        ("N", "return unknown", "names-8", "by its name", [1, 8]),
        ("N", "return text + 1", "types-15", "concat", [1, 8]),
        ("N", "return x is None", "types-14", "only an optional", [1, 8]),
        (
            "OptionalRecord",
            "return value.from",
            "types-12",
            "doesn't narrow",
            [1, 8],
        ),
        ("N", "return opt or 0", "types-15", "unwrap_or", [1, 8]),
        ("N", "return round(x, 2)", "types-26", "round(x * 100) / 100", [1, 8]),
        ("Composite", "return Child(x)", "names-11", "`child`", [1, 8]),
    ] {
        let (_, provider, mut session) = run(goal, vec![json!({"body":text}), json!({"body":"return 1"})]);
        let requests = provider.requests();
        let diagnostic = &requests[1].attempts[0].diagnostics[0];
        assert!(diagnostic.path.is_none(), "{text}: {diagnostic:?}");
        let at = diagnostic.at.as_ref().expect("place");
        assert_eq!([at.line, at.column], place);
        assert!(
            diagnostic.detail.as_deref().unwrap_or_default().contains(hint),
            "{text}: {diagnostic:?}"
        );
        assert_eq!(session.take_records()[0].cause.expect("cause").token(), rule);
        if rule == "types-12" {
            assert!(!diagnostic.detail.as_deref().unwrap_or_default().contains("`is_empty`"));
        }
    }
    // The names hint leaves out an empty list of inputs or call results, as R-SYNTH-49's does.
    for (goal, inputs, calls) in [
        ("TextValue", false, false),
        ("N", true, false),
        ("Composite", true, true),
    ] {
        let (_, provider, _) = run(goal, vec![json!({"body":"return unknown"}), json!({"body":"return 1"})]);
        let requests = provider.requests();
        let detail = requests[1].attempts[0].diagnostics[0]
            .detail
            .clone()
            .unwrap_or_default();
        assert!(
            detail.starts_with("an input or call result is read by its name"),
            "{detail}"
        );
        assert_eq!(detail.contains("the goal's inputs are: "), inputs, "{goal}: {detail}");
        assert_eq!(detail.contains("call results: "), calls, "{goal}: {detail}");
        assert!(!detail.ends_with(": "), "{detail}");
    }
}

#[test]
fn ac_synth_51_retries_quote_only_the_failing_span_and_summaries_never_do() {
    let text = "# SENTINEL-UNRELATED\nreturn x % 2\n";
    let (outcome, _, mut session) = run("N", vec![json!({"body":text})]);
    let Outcome::Failed(failure) = outcome else {
        panic!("must fail")
    };
    let learner = format!("{:?}", failure);
    assert!(learner.contains("the body used an operator the notation doesn't have (notation-7)"));
    assert!(!learner.contains("SENTINEL") && !learner.contains("x % 2") && !learner.contains("column"));
    assert_eq!(session.take_records()[0].cause.expect("cause").token(), "notation-7");
    let (_, provider, _) = run("N", vec![json!({"body":text}), json!({"body":"return 1"})]);
    let requests = provider.requests();
    let prompt = velme_synth::render_with(
        &requests[1],
        &SynthOptions {
            reply_format: ReplyFormat::Notation,
            ..SynthOptions::default()
        }
        .prompt(),
    )
    .expect("prompt");
    let feedback = &prompt.retries.last().expect("feedback").text;
    assert!(feedback.contains("VL0401 at line 2, column 10:"));
    assert!(feedback.contains("  | return x % 2"));
    assert!(!feedback.contains("SENTINEL"));
    assert!(feedback.ends_with("Return the corrected {\"body\": \"<code>\"}, with nothing else.\n"));
    insta::assert_snapshot!("notation_retry", feedback);
}

#[test]
fn ac_synth_60_request_schema_places_history_and_nonstring_body() {
    assert_eq!(ReplyFormat::from_name("notation"), Some(ReplyFormat::Notation));
    let schema = velme_synth::request_schema();
    assert_eq!(schema["$defs"]["AttemptPlace"]["properties"]["line"]["minimum"], 1);
    assert_eq!(schema["$defs"]["AttemptPlace"]["properties"]["column"]["minimum"], 1);
    assert_eq!(schema["$defs"]["AttemptPlace"]["additionalProperties"], false);
    assert_eq!(
        schema["$defs"]["AttemptDiagnostic"]["properties"]["quote"]["maxItems"],
        3
    );
    assert_eq!(
        schema["$defs"]["AttemptDiagnostic"]["properties"]["quote"]["items"]["maxLength"],
        160
    );
    let (_, provider, mut session) = run(
        "N",
        vec![
            json!({"body":{}}),
            json!({"body":"return x % 2"}),
            json!({"body":"return unknown"}),
            json!({"body":"return 1"}),
        ],
    );
    let requests = provider.requests();
    assert!(requests[2].attempts[1].diagnostics[0].at.is_some());
    assert!(requests[3].attempts[1].diagnostics[0].at.is_none());
    assert!(requests[3].attempts[1].diagnostics[0].quote.is_empty());
    let records = session.take_records();
    assert_eq!(records[0].cause.expect("cause").token(), "body-not-string");
    assert_eq!(records[0].reply_format, "notation");
    assert_eq!(records[0].shape.as_ref().expect("shape").keys, vec!["body".to_owned()]);
    assert_eq!(
        requests[1].attempts[0].diagnostics[0].code,
        velme_diagnostics::Code::IRSchemaInvalid.as_str()
    );
    assert_eq!(
        notation::quote(&"é".repeat(200), notation::Span { start: 0, end: 400 })[0]
            .chars()
            .count(),
        160
    );
    assert_eq!(
        notation::quote("a\nb\nc\nd", notation::Span { start: 0, end: 7 }).len(),
        3
    );
    assert_eq!(
        notation::quote("é\nSENTINEL-OTHER", notation::Span { start: 0, end: 2 }),
        ["é"]
    );
}

#[test]
fn ac_synth_61_a_chain_node_and_copied_list_keep_their_places() {
    let chain = lowered("return 1 < x < 2", "N").expect("chain");
    assert_eq!(chain.spans.exact("/body"), Some(notation::Span { start: 7, end: 16 }));
    let rejected = lowered("return a == [] == b", "N").expect_err("list type");
    assert_eq!(notation::locate("return a == [] == b", rejected.span.start), (1, 13));
    let chain = lowered("return x + y + 1", "N").expect("arithmetic chain");
    assert_eq!(chain.spans.at("/body/left"), Some(notation::Span { start: 7, end: 12 }));
    let chain = lowered("return 0 < x < 2", "N").expect("comparison chain");
    assert_eq!(chain.spans.at("/body/left"), Some(notation::Span { start: 7, end: 12 }));
    assert_eq!(
        chain.spans.at("/body/right"),
        Some(notation::Span { start: 11, end: 16 })
    );
    assert_eq!(chain.spans.at("/body/left/right"), chain.spans.at("/body/right/left"));
    let (_, provider, _) = run(
        "N",
        vec![json!({"body":"return 0 < x < 2"}), json!({"body":"return 1"})],
    );
    let requests = provider.requests();
    let at = requests[1].attempts[0].diagnostics[0]
        .at
        .as_ref()
        .expect("chain root finding");
    assert_eq!((at.line, at.column), (1, 8));
}

#[test]
fn ac_synth_53_replay_the_notation_fuzz_corpus_on_stable() {
    let source = include_str!("../../../tests/golden/ir/goals.velme");
    let program = program(source);
    let mut entries: Vec<_> = std::fs::read_dir(repo("fuzz/corpus/notation"))
        .expect("corpus")
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let bytes = std::fs::read(&path).expect("seed");
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (i, _) in program.goals.iter().enumerate() {
            let id = velme_sema::hir::GoalId(i);
            let Ok(request) = build_request(&program, id, source) else {
                continue;
            };
            let calls = calls(&program, id).expect("calls");
            let ir = Request {
                program: &program,
                goal: id,
                calls: &calls,
                origin: Origin::Candidate,
            };
            if let Ok(mut fresh) = notation::lower_untyped(text, &request) {
                assert_node_spans(&serde_json::to_value(&fresh.body).expect("body"), &fresh.spans);
                for pointer in velme_ir::infer_list_types(&mut fresh.body, &ir) {
                    assert!(fresh.spans.exact(&pointer).is_some(), "{pointer}");
                }
            }
            if let Ok(lowered) = notation::lower(text, &request, &ir) {
                let second = notation::lower(text, &request, &ir).expect("deterministic");
                assert_eq!(lowered.body, second.body);

                let assembled = request
                    .assemble(&serde_json::to_value(lowered.body).expect("body"))
                    .expect("assembly");
                if let Err(found) = validate_detailed(&assembled, &ir) {
                    for finding in found {
                        if finding.path.starts_with("/body") {
                            assert!(
                                lowered.spans.at(&finding.path).is_some(),
                                "{}: {}",
                                path.display(),
                                finding.path
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn ac_synth_53_every_recursive_form_and_flat_fields_fit_a_two_mib_stack() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let mut blocks = String::new();
            for depth in 0..128 {
                blocks.push_str(&format!("{}if c:\n", " ".repeat(depth)));
            }
            blocks.push_str(&format!("{}return 1\n", " ".repeat(128)));
            for depth in (0..128).rev() {
                blocks.push_str(&format!("{}else: return 0\n", " ".repeat(depth)));
            }
            for text in [
                format!("return x{}", ".f".repeat(10_000)),
                format!("return {}x{}", "floor(".repeat(128), ")".repeat(128)),
                format!("return {}1", "1 if c else ".repeat(128)),
                format!("return {}1{}", "[".repeat(128), "]".repeat(128)),
                format!("return {}xs{}", "[i for i in ".repeat(128), "]".repeat(128)),
                blocks,
            ] {
                let result = lowered(&text, "N");
                assert_eq!(result.expect_err("IR cap").rule, notation::Rule::Limit);
            }
        })
        .expect("thread")
        .join()
        .expect("no panic");
}

#[test]
fn ac_synth_53_whole_document_limits_keep_their_json_path() {
    // R-SYNTH-59's unplaced finding: copies can exceed the envelope byte cap within the notation's own caps.
    let mut expression = format!("'{}'", "a".repeat(1_000));
    for _ in 0..10 {
        expression = format!("0 < ({expression}) < 2");
    }
    let text = format!("return {expression}");
    assert!(lowered(&text, "N").is_ok());
    let (_, provider, mut session) = run("N", vec![json!({"body":text}), json!({"body":"return 1"})]);
    let requests = provider.requests();
    let finding = &requests[1].attempts[0].diagnostics[0];
    assert_eq!(session.take_records()[0].cause.expect("cause").token(), "structure-1");
    assert!(finding.at.is_none() && finding.quote.is_empty());
    assert!(finding.path.is_none());
    assert!(finding.detail.as_deref().unwrap_or_default().contains("whole program"));
}

fn assert_node_spans(body: &serde_json::Value, spans: &notation::Spans) {
    let mut pending = vec![("/body".to_owned(), body)];
    while let Some((path, value)) = pending.pop() {
        match value {
            serde_json::Value::Object(fields) => {
                if fields.contains_key("kind") {
                    assert!(spans.exact(&path).is_some(), "missing node span: {path}");
                }
                for (key, value) in fields {
                    if !(fields.get("kind").and_then(serde_json::Value::as_str) == Some("literal") && key == "value") {
                        pending.push((format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")), value));
                    }
                }
            }
            serde_json::Value::Array(items) => {
                pending.extend(items.iter().enumerate().map(|(i, v)| (format!("{path}/{i}"), v)));
            }
            _ => {}
        }
    }
}

#[test]
fn ac_synth_51_review_rejections_keep_phase_order_places_and_hints() {
    for (text, rule, at, hint) in [
        ("return filter(xs)", "notation-10", (1, 8), "comprehension"),
        ("return map(xs)", "notation-10", (1, 8), "comprehension"),
        (
            "x = 1\nx = 2\nreturn sum(1 + (y for y in xs))",
            "notation-8",
            (3, 19),
            "",
        ),
        (
            "return next((v for v in xs if v > 0), (w for w in xs))",
            "notation-8",
            (1, 42),
            "",
        ),
        ("return [max(1) for xs in xs]", "notation-10", (1, 9), "maximum"),
        ("return all(max(1) for xs in xs)", "notation-10", (1, 12), "maximum"),
        ("return concat(\"a\" \"b\")", "notation-3", (1, 19), "concat"),
        ("return f(1,\n", "notation-13", (1, 12), ""),
        (
            "return [a for a, b in xs]",
            "notation-8",
            (1, 16),
            "write a list `[a, b]`",
        ),
        (
            "if a, b:\n    return 1\nreturn 2",
            "notation-8",
            (1, 5),
            "write a list `[a, b]`",
        ),
        (
            "return x in (1, 2) + ys",
            "notation-8",
            (1, 13),
            "write a list `[a, b]`",
        ),
        (
            "return x in (1, 2) == c",
            "notation-8",
            (1, 13),
            "write a list `[a, b]`",
        ),
        // A form the notation lacks inside a call of a Python built-in gets that built-in's hint.
        ("return max(v.a for v in xs)", "notation-8", (1, 16), "maximum"),
        ("return max(xs, key=lambda v: v)", "notation-8", (1, 20), "maximum"),
        ("return int(xs, key=lambda v: v)", "notation-8", (1, 20), "floor(x)"),
        ("return float(xs, key=lambda v: v)", "notation-8", (1, 22), "next(…)"),
        ("return f(v for v in xs)", "notation-8", (1, 12), "next(…)"),
        // A nested non-built-in call settled the rejection first: the outer built-in's hint doesn't replace it.
        ("return max(f(v for v in xs))", "notation-8", (1, 16), "next(…)"),
        // A bare generator with other arguments is the form in the wrong shape.
        (
            "return all(v > 0 for v in xs, 1)",
            "notation-9",
            (1, 8),
            "all(c for x in xs)",
        ),
        (
            "return sum(v for v in xs, 0)",
            "notation-9",
            (1, 8),
            "sum(e for x in xs)",
        ),
        // A lex error after a keyword doesn't beat the syntax error at it.
        ("return f(in $)", "notation-13", (1, 10), ""),
    ] {
        let error = lowered(text, "N").expect_err(text);
        assert_eq!(error.rule.id(), rule, "{text}: {error:?}");
        assert_eq!(notation::locate(text, error.span.start), at, "{text}");
        assert!(
            error.hint.as_deref().unwrap_or_default().contains(hint),
            "{text}: {error:?}"
        );
    }
}

#[test]
fn ac_synth_50_keyword_followed_by_text_is_not_a_string_prefix() {
    assert_eq!(body("return\"a\"", "TextValue"), body("return \"a\"", "TextValue"));
}

#[test]
fn ac_synth_61_exact_spans_include_untyped_lists_and_each_node() {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "N"), SOURCE).expect("request");
    let fresh = notation::lower_untyped("return a == [] == b", &request).expect("raw lowering");
    assert_node_spans(&serde_json::to_value(&fresh.body).expect("body"), &fresh.spans);
    assert!(fresh.spans.exact("/body/missing").is_none());
    let ir = Request {
        program: &program,
        goal: goal_id(&program, "N"),
        calls: &[],
        origin: Origin::Candidate,
    };
    let mut node = fresh.body;
    let untyped = velme_ir::infer_list_types(&mut node, &ir);
    assert!(!untyped.is_empty());
    for path in untyped {
        assert!(fresh.spans.exact(&path).is_some(), "{path}");
    }
}

#[test]
fn ac_synth_53_comprehension_scope_growth_is_linear() {
    let program = program(SOURCE);
    let request = build_request(&program, goal_id(&program, "N"), SOURCE).expect("request");
    let measure = |n| {
        let text = (0..n).map(|i| format!("a{i}=[v for v in xs]\n")).collect::<String>() + "return 1";
        let mut times = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let result = notation::lower_untyped(&text, &request).expect("lowered");
            times.push(start.elapsed());
            drop(result);
        }
        times.sort();
        times[2]
    };
    let small = measure(1450);
    let large = measure(2900);
    assert!(large < small.mul_f64(3.3), "1450: {small:?}; 2900: {large:?}");
}
