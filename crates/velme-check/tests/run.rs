//! Check and example evaluation on the reference interpreter, and their failure reports (`language/13` §4–5,
//! `language/12` R-GOAL-22).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::limits::MAX_FUEL;
use velme_builtins::{Number, Value};
use velme_check::{Checked, GoalChecks, Invocation, Part};
use velme_diagnostics::render::render_human;
use velme_diagnostics::{Code, Diagnostic};
use velme_interp::run;
use velme_ir::{SHOWN_ITEMS, decode_str};
use velme_sema::hir::Program;
use velme_test_support::{goal_id, program, read, repo, valid_ir};

const SOURCE: &str = r#"language: velme/0.1

type Player:
    name: Text
    score: Number

type Summary:
    score: Number
    rank: Number

type Ball:
    bounce: Number

goal CalculateScore(player: Player) -> Number:
    plan: "The player's score."

goal CalculateRank(player: Player) -> Number:
    plan: "The player's rank."

goal BuildPlayerSummary(player: Player) -> Summary:
    call:
        score = CalculateScore(player)
        rank = CalculateRank(player)
    plan: "Build the summary from the score and rank."
    check:
        - result.score == score
        - result.rank == rank

goal MakeBalls(count: Number) -> List<Ball>:
    plan: "Make count balls."
    check:
        - every b in result has b.bounce >= 5

goal Quantifiers(xs: List<Number>) -> Number:
    plan: "Anything."
    check:
        - every x in xs has false
        - not (some x in xs has true)

goal FindBest(players: List<Player>) -> Player?:
    plan: "The best player."
    check:
        - if players is empty then result is empty
        - result is empty or result.score >= 0

goal Average(total: Number, count: Number) -> Number:
    plan: "The average."
    check:
        - total / count > 1
        - result > 100
        - result >= 0

goal Bounded(xs: List<Number>) -> Number:
    plan: "Anything."
    check:
        - every x in xs has x > 0 and x < 10

goal Groups(gs: List<List<Number>>) -> Number:
    plan: "Anything."
    check:
        - every g in gs has g.length < 2 and (every x in g has x > 0)
        - some g in gs has every x in g has x > 0

goal Tenths(xs: List<Number>) -> Number:
    plan: "Anything."
    check:
        - every x in xs has 10 / x > 1

goal Ranges(n: Number) -> Number:
    plan: "Anything."
    check:
        - sum(range(n)) >= 0
"#;

/// The JSON `text` decoded as a value of `ty`.
fn value(program: &Program, text: &str, ty: &velme_sema::hir::Type) -> Value {
    decode_str(text, ty, program).expect("decodes")
}

/// An invocation of `goal` with JSON inputs, bindings and result, that has spent `fuel`.
fn invocation(program: &Program, goal: &str, inputs: &[&str], bindings: &[&str], result: &str) -> Invocation {
    let decl = &program.goals[goal_id(program, goal).0];
    Invocation {
        inputs: decl
            .params
            .iter()
            .zip(inputs)
            .map(|(p, t)| value(program, t, &p.ty))
            .collect(),
        bindings: decl
            .bindings
            .iter()
            .zip(bindings)
            .map(|(b, t)| value(program, t, &b.ty))
            .collect(),
        result: value(program, result, &decl.output),
        fuel: 0,
    }
}

fn checks<'p>(program: &'p Program, goal: &str) -> GoalChecks<'p> {
    GoalChecks::new(program, goal_id(program, goal), SOURCE).expect("the checks lower")
}

fn check(goal: &str, inputs: &[&str], bindings: &[&str], result: &str) -> Checked {
    let program = program(SOURCE);
    let invocation = invocation(&program, goal, inputs, bindings, result);
    checks(&program, goal).run(&invocation).expect("the checks run")
}

fn failures(checked: &Checked) -> Vec<Diagnostic> {
    checked.failures()
}

/// The part `text` of the check item `item`, with `value`.
fn part(item: &str, text: &str, value: Option<Value>) -> Part {
    let start = SOURCE.find(item).expect("in the source") + item.find(text).expect("in the item");
    Part {
        span: velme_diagnostics::Span::new(start, start + text.len()),
        text: text.to_owned(),
        value,
    }
}

const LINA: &str = r#"{"name": "Lina", "score": 820}"#;

#[test]
fn ac_chk_04_failed_equality_shows_expected_received_and_bindings() {
    // AC-RDM-06 on hand-written values: the assertion, Expected/Received and the values behind them.
    let checked = check(
        "BuildPlayerSummary",
        &[LINA],
        &["820", "4"],
        r#"{"score": 820, "rank": 3}"#,
    );
    let diags = failures(&checked);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    let d = &diags[0];
    assert_eq!(d.code, Code::CheckFailed);
    assert_eq!(
        d.message,
        "`BuildPlayerSummary` didn't pass its check: `result.rank == rank`."
    );
    assert_eq!(&SOURCE[d.span.start..d.span.end], "result.rank == rank");
    assert_eq!(
        d.notes,
        [
            "Expected: 4",
            "Received: 3",
            "`result.rank` = 3",
            "`rank` = 4",
            r#"input `player` = {"name":"Lina","score":820}"#,
            "call `score` = 820",
            "call `rank` = 4",
        ]
    );
}

#[test]
fn ac_chk_05_quantifiers_over_an_empty_list() {
    let checked = check("Quantifiers", &["[]"], &[], "0");
    assert!(checked.failures().is_empty(), "{:#?}", checked.failures());
}

#[test]
fn ac_chk_06_every_reports_the_first_counterexample_and_stops_there() {
    let result = r#"[{"bounce": 6}, {"bounce": 7}, {"bounce": 3}, {"bounce": 2}]"#;
    let checked = check("MakeBalls", &["4"], &[], result);
    let diags = failures(&checked);
    assert_eq!(
        diags[0].notes,
        [
            r#"`result[2]` = {"bounce":3} fails `b.bounce >= 5`"#,
            r#"`result` = [{"bounce":6},{"bounce":7},{"bounce":3},{"bounce":2}]"#,
            "`b.bounce` = 3",
            "input `count` = 4",
        ]
    );
    // `all`, `result`, then three elements of a visit and four nodes each; the fourth is never visited.
    assert_eq!(checked.fuel, 2 + 3 * 5);
}

#[test]
fn ac_chk_07_implication_with_a_false_condition_holds_without_its_then_side() {
    const ITEM: &str = "if players is empty then result is empty";
    let program = program(SOURCE);
    let players = format!("[{LINA}]");
    let invocation = invocation(&program, "FindBest", &[&players], &[], "null");
    let checks = checks(&program, "FindBest");
    // `result is empty or …` narrows `result` in its right side, which is skipped when it is empty.
    assert!(checks.run(&invocation).expect("runs").failures().is_empty());
    let decl = &program.goals[goal_id(&program, "FindBest").0];
    let players = value(&program, &players, &decl.params[0].ty);
    assert_eq!(
        checks.explain(0, &invocation).expect("explains"),
        [
            part(ITEM, "players", Some(players)),
            part(ITEM, "result is empty", None)
        ]
    );
}

#[test]
fn ac_chk_08_a_value_error_fails_the_item_with_its_cause() {
    let checked = check("Average", &["10", "0"], &[], "5");
    let diags = failures(&checked);
    let d = &diags[0];
    assert_eq!(d.code, Code::CheckFailed);
    assert_eq!(d.message, "`Average` didn't pass its check: `total / count > 1`.");
    assert!(
        d.notes.contains(
            &"could not evaluate `total / count`: it tried to divide 10 by 0, which has no answer [VL0602]".to_owned()
        ),
        "{:#?}",
        d.notes
    );
}

#[test]
fn ac_chk_09_every_failing_item_is_reported_in_source_order() {
    let checked = check("Average", &["10", "0"], &[], "5");
    let messages: Vec<_> = failures(&checked).into_iter().map(|d| d.message).collect();
    assert_eq!(
        messages,
        [
            "`Average` didn't pass its check: `total / count > 1`.",
            "`Average` didn't pass its check: `result > 100`.",
        ]
    );
    assert!(checked.items[2].failure.is_none());
}

#[test]
fn ac_chk_10_running_out_of_fuel_in_a_check_is_a_budget_failure() {
    let program = program(SOURCE);
    let mut invocation = invocation(&program, "MakeBalls", &["2"], &[], r#"[{"bounce": 6}, {"bounce": 7}]"#);
    // The body spent all but a few units of the invocation's budget.
    invocation.fuel = MAX_FUEL - 5;
    let diag = checks(&program, "MakeBalls").run(&invocation).expect_err("stopped");
    assert_eq!(diag.code, Code::BudgetExceeded);
    assert_eq!(diag.message, "`MakeBalls` took too many steps and was stopped.");
    // The goal's limit, not what was left of it.
    assert_eq!(
        diag.notes,
        [
            format!("it may take {MAX_FUEL} steps (fuel)"),
            "this happened at item 0 of a list (counting from 0)".to_owned()
        ]
    );
}

/// A list past its limit stops the checks like running out of fuel does (R-CHK-08), instead of failing the item.
#[test]
fn a_list_past_its_limit_in_a_check_is_a_budget_failure() {
    let program = program(SOURCE);
    let invocation = invocation(&program, "Ranges", &["20000"], &[], "0");
    let diag = checks(&program, "Ranges").run(&invocation).expect_err("stopped");
    assert_eq!(diag.code, Code::SizeLimitExceeded);
}

/// A short-circuited operand inside a quantifier's body shows as not evaluated, not with an earlier element's value.
#[test]
fn values_are_those_of_the_deciding_element() {
    let diags = failures(&check("Bounded", &["[5, -1]"], &[], "0"));
    assert_eq!(
        diags[0].notes,
        [
            "`xs[1]` = -1 fails `x > 0 and x < 10`",
            "`xs` = [5,-1]",
            "`x` = -1",
            "`x < 10` was not evaluated",
            "input `xs` = [5,-1]",
        ]
    );
    // The inner quantifier ran for `[1]`, but not for `[5,6,7]`, which decided the outer one.
    let checked = check("Groups", &["[[1], [5, 6, 7]]"], &[], "0");
    assert_eq!(
        checked.items[0].failure.as_ref().expect("fails").notes,
        [
            "`gs[1]` = [5,6,7] fails `g.length < 2 and (every x in g has x > 0)`",
            "`gs` = [[1],[5,6,7]]",
            "`g.length` = 3",
            "`g` = [5,6,7]",
            "`(every x in g has x > 0)` was not evaluated",
            "input `gs` = [[1],[5,6,7]]",
        ]
    );
}

/// An inner quantifier's element is not reported when the outer quantifier decided nothing.
#[test]
fn inner_quantifiers_report_only_under_a_deciding_one() {
    let checked = check("Groups", &["[[1, -1], [-2]]"], &[], "0");
    let notes = &checked.items[1].failure.as_ref().expect("fails").notes;
    assert_eq!(notes, &["`gs` = [[1,-1],[-2]]", "input `gs` = [[1,-1],[-2]]"]);
}

#[test]
fn a_failure_inside_a_quantifier_names_its_element() {
    let diags = failures(&check("Tenths", &["[5, 0]"], &[], "0"));
    let notes = &diags[0].notes;
    let cause = "could not evaluate `10 / x`: it tried to divide 10 by 0, which has no answer [VL0602]";
    assert!(notes.contains(&cause.to_owned()), "{notes:#?}");
    assert!(notes.contains(&"this happened at `xs[1]` = 0".to_owned()), "{notes:#?}");
}

#[test]
fn checks_hold_on_a_right_answer() {
    let checked = check(
        "BuildPlayerSummary",
        &[LINA],
        &["820", "3"],
        r#"{"score": 820, "rank": 3}"#,
    );
    assert!(checked.failures().is_empty());
    assert_eq!(checked.items.len(), 2);
}

#[test]
fn long_values_are_cut_for_display_but_kept_whole() {
    let balls = format!("[{}]", vec![r#"{"bounce": 1}"#; SHOWN_ITEMS + 5].join(", "));
    let checked = check("MakeBalls", &["20"], &[], &balls);
    let notes = &checked.items[0].failure.as_ref().expect("fails").notes;
    let result = notes.iter().find(|n| n.starts_with("`result` = ")).expect("result");
    assert!(result.ends_with(",…(+5 items)]"), "{result}");
    // The part keeps the whole value, for the trace.
    let part = checked.items[0]
        .parts
        .iter()
        .find(|p| p.text == "result")
        .expect("result");
    assert!(matches!(&part.value, Some(Value::List(items)) if items.len() == SHOWN_ITEMS + 5));
}

// ---- a hand-written wrong implementation ----

/// Runs `Double`'s examples through `run_goal`, then its checks on each invocation, as the runtime does (R-GOAL-22).
fn examples(program: &Program, source: &str, run_goal: impl Fn(Vec<Value>) -> Invocation) -> Vec<Diagnostic> {
    let checks = GoalChecks::new(program, goal_id(program, "Double"), source).expect("lowers");
    let mut diags = Vec::new();
    for example in checks.examples() {
        let invocation = run_goal(example.args.clone());
        diags.extend(checks.judge_example(example, &invocation));
        diags.extend(checks.run(&invocation).expect("the checks run").failures());
    }
    diags
}

/// `tests/golden/checks/double.json` is `x + 1` for `Double`: its examples and checks fail with the values behind them
/// (AC-RDM-06, R-GOAL-22).
#[test]
fn ac_rdm_06_wrong_ir_fails_its_examples_and_checks_with_values() {
    let source = read(&repo("tests/golden/checks/double.velme"));
    let program = program(&source);
    let ir = valid_ir(&program, &read(&repo("tests/golden/checks/double.json")));
    let diags = examples(&program, &source, |inputs| {
        let output = run(&ir, inputs.clone(), Vec::new(), MAX_FUEL).expect("runs");
        Invocation {
            inputs,
            bindings: Vec::new(),
            result: output.value,
            fuel: output.fuel,
        }
    });
    assert_eq!(
        diags.iter().map(|d| d.code).collect::<Vec<_>>(),
        [
            Code::ExampleFailed,
            Code::CheckFailed,
            Code::ExampleFailed,
            Code::CheckFailed
        ]
    );
    assert_eq!(
        diags[0].message,
        "For Double(2), `Double` gave 3 but the example expects 4."
    );
    insta::assert_snapshot!(render_human(&diags, "double.velme", Some(&source), false));
}

#[test]
fn right_ir_passes_its_examples() {
    let source = read(&repo("tests/golden/checks/double.velme"));
    let program = program(&source);
    let diags = examples(&program, &source, |inputs| {
        let Some(Value::Number(x)) = inputs.first() else {
            panic!("one number")
        };
        let result = Value::Number(x.checked_mul(Number::from(2i64)).expect("fits"));
        Invocation {
            inputs,
            bindings: Vec::new(),
            result,
            fuel: 0,
        }
    });
    assert!(diags.is_empty(), "{diags:#?}");
}
