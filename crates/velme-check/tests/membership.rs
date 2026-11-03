//! Domain membership (`language/13` R-CHK-15, `runtime/30` R-RUN-26–28): closed sets and complete conditions, the deep
//! walk's paths and exact fuel, and the source-literal pass (`language/11` R-TYP-31).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::{Number, Value};
use velme_check::{DOMAIN_VISIT_FUEL, GoalChecks, Invocation, Membership, Outside, Refused, check_literals};
use velme_diagnostics::Code;
use velme_interp::{Budget, Error, Limits, Spent};
use velme_sema::hir::{Goal, Program};
use velme_test_support::{goal_id, program};

const SOURCE: &str = "language: velme/0.2

type Risk = Text one of \"LOW\", \"HIGH\"
type Score = Number where value >= 0
type Lucky = Number where value != 13
type Ratio = Number where 10 / value > 1
type Pair where value.low <= value.high:
    low: Number
    high: Number
type Applicant:
    name: Text
    risk: Risk
    scores: List<Score>
    backup: Score?
    plain: Number

goal Assess(applicant: Applicant, pair: Pair) -> List<Risk?>:
    plan: \"Rate each score.\"

goal Plain(xs: List<Number>, name: Text) -> Number:
    plan: \"Count them.\"

goal Pick(n: Number) -> Score:
    plan: \"Keep it.\"

goal Lucky13(n: Number) -> Lucky:
    plan: \"Keep it.\"

goal Divide(n: Number) -> Ratio:
    plan: \"Keep it.\"

goal Many(n: Number) -> List<Score>:
    plan: \"Repeat it.\"
";

fn n(x: i64) -> Value {
    Value::from(Number::from(x))
}

fn goal<'p>(program: &'p Program, name: &str) -> &'p Goal {
    &program.goals[goal_id(program, name).0]
}

fn applicant(risk: &str, scores: &[i64], backup: Option<i64>) -> Value {
    Value::record(
        "Applicant",
        vec![
            ("name".to_owned(), Value::text("Ada")),
            ("risk".to_owned(), Value::text(risk)),
            ("scores".to_owned(), Value::list(scores.iter().map(|s| n(*s)).collect())),
            ("backup".to_owned(), backup.map_or(Value::Nothing, n)),
            ("plain".to_owned(), n(-1)),
        ],
    )
}

fn pair(low: i64, high: i64) -> Value {
    Value::record("Pair", vec![("low".to_owned(), n(low)), ("high".to_owned(), n(high))])
}

fn budget(fuel: u64) -> Budget {
    Budget::new(Limits {
        fuel,
        memory: Limits::SYSTEM.memory,
    })
}

/// The output walk of goal `name` on `value`, within `fuel`.
fn output(name: &str, value: &Value, fuel: u64) -> (Result<(), Refused>, Spent) {
    let program = program(SOURCE);
    let membership = Membership::new(&program, SOURCE).expect("membership");
    membership.output(goal(&program, name), value, budget(fuel))
}

fn outside(refused: Result<(), Refused>) -> Outside {
    match refused {
        Err(Refused::Outside(outside)) => outside,
        other => panic!("not a non-member: {other:?}"),
    }
}

/// AC-CHK-16: a residual condition that is false, or that divides by zero, rejects membership with its cause; running
/// out of fuel in it stays `VL0601`.
#[test]
fn ac_chk_16_a_false_or_failing_condition_rejects_and_fuel_stays_vl0601() {
    let lucky = outside(output("Lucky13", &n(13), u64::MAX).0);
    assert_eq!((lucky.domain.as_str(), lucky.path.as_str()), ("Lucky", "result"));
    assert_eq!(lucky.reason, "`value != 13` is false");
    assert_eq!(lucky.cause, None);
    assert_eq!(output("Lucky13", &n(12), u64::MAX).0, Ok(()));

    let ratio = outside(output("Divide", &n(0), u64::MAX).0);
    assert_eq!(ratio.cause, Some(Code::ArithmeticError));
    assert!(
        ratio.reason.contains("`10 / value > 1` tried to divide 10 by 0"),
        "{}",
        ratio.reason
    );
    assert_eq!(output("Divide", &n(5), u64::MAX).0, Ok(()));

    // The visit of the domain is paid, then the condition runs out.
    match output("Lucky13", &n(12), DOMAIN_VISIT_FUEL + 1).0 {
        Err(Refused::Stopped(failure)) => {
            assert!(matches!(failure.error, Error::OutOfFuel { .. }), "{failure:?}");
            assert_eq!(failure.code(), Code::BudgetExceeded);
        }
        other => panic!("{other:?}"),
    }
}

/// AC-RUN-13: invalid nested record fields, list items and present optionals give the first path, depth first in
/// declaration and index order, while `nothing` passes an optional domain.
#[test]
fn ac_run_13_the_first_failing_path_is_depth_first() {
    let program = program(SOURCE);
    let membership = Membership::new(&program, SOURCE).expect("membership");
    let assess = goal(&program, "Assess");
    let walk = |a: Value, p: Value| membership.inputs(assess, &[a, p], Budget::new(Limits::SYSTEM)).0;
    let first = |a: Value, p: Value| match walk(a, p) {
        Err((param, Refused::Outside(outside))) => (param, outside.path, outside.domain),
        other => panic!("{other:?}"),
    };
    assert_eq!(walk(applicant("LOW", &[1, 2], None), pair(1, 2)), Ok(()));
    assert_eq!(walk(applicant("HIGH", &[], Some(0)), pair(2, 2)), Ok(()));
    assert_eq!(
        first(applicant("MID", &[-1], Some(-1)), pair(1, 2)),
        (0, "applicant.risk".to_owned(), "Risk".to_owned())
    );
    assert_eq!(
        first(applicant("LOW", &[1, -1, -2], Some(-1)), pair(1, 2)),
        (0, "applicant.scores[1]".to_owned(), "Score".to_owned())
    );
    assert_eq!(
        first(applicant("LOW", &[], Some(-1)), pair(1, 2)),
        (0, "applicant.backup".to_owned(), "Score".to_owned())
    );
    // A cross-field record condition.
    assert_eq!(
        first(applicant("LOW", &[], None), pair(3, 2)),
        (1, "pair".to_owned(), "Pair".to_owned())
    );
    let refused = outside(output("Many", &Value::list(vec![n(1), n(-1)]), u64::MAX).0);
    assert_eq!(refused.path, "result[1]");
    // A simple constraint it breaks says so, as the static check does.
    assert_eq!(refused.reason, "it must be at least 0");
    let risky = outside(output("Lucky13", &n(13), u64::MAX).0);
    assert_eq!(risky.path, "result");
}

/// AC-RUN-15: a domain-free value costs nothing; every visited pair costs `DOMAIN_VISIT_FUEL`, a domain's base
/// included, repeated values each pay, a closed set costs what `contains` would and allocates nothing, and running out at
/// any debit is `VL0601`.
#[test]
fn ac_run_15_membership_fuel_is_exact() {
    let program = program(SOURCE);
    let membership = Membership::new(&program, SOURCE).expect("membership");
    let plain = goal(&program, "Plain");
    let (walked, spent) = membership.inputs(
        plain,
        &[Value::list(vec![n(1), n(2)]), Value::text("x")],
        Budget::new(Limits::SYSTEM),
    );
    assert_eq!((walked, spent), (Ok(()), Spent::default()));

    // `value >= 0` evaluates its node and its two operands (R-RUN-04).
    let predicate = 3;
    let (walked, spent) = output("Pick", &n(4), u64::MAX);
    assert_eq!((walked, spent.fuel), (Ok(()), 2 * DOMAIN_VISIT_FUEL + predicate));
    // The list, then each item's domain and base; the same value twice pays twice.
    let shared = n(7);
    let many = Value::list(vec![shared.clone(), shared.clone(), shared]);
    let each = 2 * DOMAIN_VISIT_FUEL + predicate;
    let (walked, spent) = output("Many", &many, u64::MAX);
    assert_eq!((walked, spent.fuel), (Ok(()), DOMAIN_VISIT_FUEL + 3 * each));

    // `Risk` is `"LOW", "HIGH"`: `contains`'s unit, then for "HIGH" the member "LOW" (1, its length differs) and
    // "HIGH" (1 and a block); "LOW" stops at the first.
    let assess = goal(&program, "Assess");
    let run =
        |risk: &str, fuel: u64| membership.inputs(assess, &[applicant(risk, &[], None), pair(1, 1)], budget(fuel));
    let (walked, high) = run("HIGH", u64::MAX);
    assert_eq!(walked, Ok(()));
    let (_, low) = run("LOW", u64::MAX);
    assert_eq!(high.fuel - low.fuel, 1, "one more member scanned, whose length differs");
    assert_eq!(high.memory, 0, "a closed set builds no list");
    // The applicant (1), `risk`'s domain and base (2) and set (1 + 1 + 2), `scores` (1), `backup` (1), then `pair`'s
    // domain and base (2) and its condition.
    let pair_condition = high.fuel - (1 + 2 + 4 + 1 + 1 + 2);
    assert_eq!(pair_condition, 5, "two field reads, their records and the comparison");

    // Out of fuel at any debit is `VL0601`; with exactly enough, it passes.
    assert_eq!(run("HIGH", high.fuel).0, Ok(()));
    for fuel in 0..high.fuel {
        match run("HIGH", fuel).0 {
            Err((_, Refused::Stopped(failure))) => assert_eq!(failure.code(), Code::BudgetExceeded, "{fuel}"),
            other => panic!("{fuel}: {other:?}"),
        }
    }
}

/// AC-TYP-30: a source literal violating a residual Number or Text condition, or a cross-field record one, is `VL0204`
/// with its cause, nested example and call-argument literals too; valid constants pass and a value error rejects. No
/// call-boundary check is added: a path argument is left to its producer's output check.
#[test]
fn ac_typ_30_source_literals_are_held_to_the_whole_condition() {
    let source = |body: &str| {
        format!(
            "language: velme/0.2

type Lucky = Number where value != 13
type Name = Text where value != \"nobody\"
type Ratio = Number where 10 / value > 1
type Pair where value.low <= value.high:
    low: Number
    high: Number
type Box:
    pair: Pair
    names: List<Name?>

goal Keep(n: Lucky, box: Box) -> Ratio:
    plan: \"Keep it.\"
{body}"
        )
    };
    let found = |body: &str| {
        let text = source(body);
        let program = program(&text);
        check_literals(&program, &text)
    };
    let fine = "    examples:
        - Keep(12, Box(pair: Pair(low: 1, high: 2), names: [\"ada\", nothing])) == 5
";
    assert_eq!(found(fine), Vec::new());

    let bad = "    examples:
        - Keep(13, Box(pair: Pair(low: 3, high: 2), names: [nothing, \"nobody\"])) == 0
";
    let diags = found(bad);
    let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "`13` isn't a valid `Lucky`: `value != 13` is false.",
            "`Pair(low: 3, high: 2)` isn't a valid `Pair`: `value.low <= value.high` is false.",
            "`\"nobody\"` isn't a valid `Name`: `value != \"nobody\"` is false.",
            "`0` isn't a valid `Ratio`: `10 / value > 1` tried to divide 10 by 0, which has no answer.",
        ],
        "{diags:#?}"
    );
    assert!(diags.iter().all(|d| d.code == Code::TypeMismatch));
    assert_eq!(diags[3].notes, ["cause: VL0602"]);

    // A call argument, in a wired goal: a literal is checked, a path isn't.
    let wired = "
goal Twice(box: Box) -> Ratio:
    call:
        a = Keep(13, box)
        result = Keep(12, box)
";
    let diags = found(wired);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].message, "`13` isn't a valid `Lucky`: `value != 13` is false.");
}

/// AC-TYP-31 (D4): a record-domain constructor in a check builds the underlying record and is not tested there, while
/// the same value is tested at an input, an output and a source literal.
#[test]
fn ac_typ_31_a_constructor_in_a_check_is_not_a_membership_position() {
    let text = "language: velme/0.2

type Pair where value.low <= value.high:
    low: Number
    high: Number

goal Swap(p: Pair) -> Pair:
    plan: \"Keep it.\"
    check:
        - Pair(low: 3, high: 2).low == 3
";
    let program = program(text);
    assert_eq!(check_literals(&program, text), Vec::new());
    let id = goal_id(&program, "Swap");
    let checks = GoalChecks::new(&program, id, text).expect("checks");
    let invocation = Invocation {
        inputs: vec![pair(1, 2)],
        bindings: Vec::new(),
        result: pair(1, 2),
        fuel: 0,
        memory: 0,
    };
    assert_eq!(checks.run(&invocation).expect("checks run").failures(), Vec::new());

    let membership = Membership::new(&program, text).expect("membership");
    let swap = &program.goals[id.0];
    let refused = membership.inputs(swap, &[pair(3, 2)], Budget::new(Limits::SYSTEM)).0;
    assert!(matches!(refused, Err((0, Refused::Outside(_)))), "{refused:?}");
    let refused = membership.output(swap, &pair(3, 2), Budget::new(Limits::SYSTEM)).0;
    assert!(matches!(refused, Err(Refused::Outside(_))), "{refused:?}");

    let literal = format!("{text}    examples:\n        - Swap(Pair(low: 3, high: 2)) == Pair(low: 1, high: 2)\n");
    let program = velme_test_support::program(&literal);
    let diags = check_literals(&program, &literal);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].code, Code::TypeMismatch);
}

/// AC-TYP-30: a predicate's budget failure on a literal keeps its code: a fresh system budget runs out.
#[test]
fn ac_typ_30_a_literal_predicate_out_of_budget_keeps_its_code() {
    let text = "language: velme/0.2

type Busy = Number where every i in range(10000) has every j in range(10000) has i + j >= value

goal Keep(n: Busy) -> Number:
    plan: \"Keep it.\"
    examples:
        - Keep(0) == 0
";
    let program = program(text);
    let diags = check_literals(&program, text);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert!(
        matches!(diags[0].code, Code::BudgetExceeded | Code::MemoryLimitExceeded),
        "{diags:#?}"
    );
}

/// AC-RUN-14: closed sets, Number bounds and `is whole`, Text length and residual cross-field conditions each reject a
/// non-member output, with the static check's reason where a simple constraint fails.
#[test]
fn ac_run_14_each_kind_of_domain_rejects_its_non_members() {
    let text = "language: velme/0.2

type Risk = Text one of \"LOW\", \"HIGH\"
type Count = Number where value is whole and value < 10
type Code = Text where value.length <= 3
type Pair where value.low <= value.high:
    low: Number
    high: Number

goal Rate(n: Number) -> Risk:
    plan: \"Rate n.\"

goal Tally(n: Number) -> Count:
    plan: \"Count n.\"

goal Name(n: Number) -> Code:
    plan: \"Name n.\"

goal Span(n: Number) -> Pair:
    plan: \"Span n.\"
";
    let program = program(text);
    let membership = Membership::new(&program, text).expect("membership");
    let reason = |name: &str, value: Value| match membership
        .output(goal(&program, name), &value, Budget::new(Limits::SYSTEM))
        .0
    {
        Ok(()) => None,
        Err(Refused::Outside(outside)) => Some(outside.reason),
        Err(other) => panic!("{other:?}"),
    };
    assert_eq!(
        reason("Rate", Value::text("low")),
        Some("it isn't one of its members".to_owned())
    );
    assert_eq!(reason("Rate", Value::text("LOW")), None);
    let half = Value::from(Number::parse("2.5").expect("a number"));
    assert_eq!(reason("Tally", half), Some("it must be a whole number".to_owned()));
    assert_eq!(reason("Tally", n(10)), Some("it must be less than 10".to_owned()));
    assert_eq!(reason("Tally", n(9)), None);
    assert_eq!(
        reason("Name", Value::text("four")),
        Some("its length must be at most 3".to_owned())
    );
    assert_eq!(reason("Name", Value::text("abc")), None);
    assert_eq!(
        reason("Span", pair(2, 1)),
        Some("`value.low <= value.high` is false".to_owned())
    );
    assert_eq!(reason("Span", pair(1, 1)), None);
}

const GATE: &str = "language: velme/0.2

type Even = Number where floor(value / 2) * 2 == value
type Age = Number where value >= 0 and value <= 120
type Person:
    age: Age
    n: Even
    ns: List<Even>
type Pair where Person(age: 1, n: {pair_n}, ns: [2]).age == 1:
    a: Number

goal Make(x: Number) -> Person:
    plan: \"A person.\"
    check:
{check}
";

fn gate(pair_n: i64, check: &str) -> Vec<velme_diagnostics::Diagnostic> {
    let text = GATE.replace("{pair_n}", &pair_n.to_string()).replace("{check}", check);
    let program = program(&text);
    check_literals(&program, &text)
}

/// Gate F3 (D-171): the constant fields and items of a record or list literal in a check meet the domain's full
/// condition, not only its simple constraints; a non-constant sibling doesn't hide them.
#[test]
fn gate_f3_check_literals_meet_the_full_condition() {
    assert_eq!(
        gate(2, "        - result != Person(age: 5, n: 4, ns: [2, 6])"),
        Vec::new()
    );
    for (check, bad) in [
        ("        - result != Person(age: 5, n: 3, ns: [])", "3"),
        ("        - result != Person(age: 5, n: x, ns: [2, 5])", "5"),
        ("        - result != Person(age: 5, n: 3, ns: [x])", "3"),
    ] {
        let diags = gate(2, check);
        assert_eq!(diags.len(), 1, "{check}: {diags:#?}");
        assert_eq!(diags[0].code, Code::TypeMismatch);
        assert!(
            diags[0].message.starts_with(&format!("`{bad}` isn\'t a valid `Even`")),
            "{}",
            diags[0].message
        );
    }
    // Several offenders are all reported, in source order.
    let diags = gate(2, "        - result != Person(age: 5, n: 3, ns: [1])");
    assert_eq!(diags.len(), 2, "{diags:#?}");
}

/// Gate F3 (D-171): the same in a domain condition is `VL0209`, naming the domain.
#[test]
fn gate_f3_condition_literals_meet_the_full_condition() {
    let diags = gate(3, "        - result.age == result.age");
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].code, Code::InvalidDomain);
    assert!(
        diags[0]
            .message
            .starts_with("Domain `Pair` is invalid: its condition has a problem: `3` isn\'t a valid `Even`"),
        "{}",
        diags[0].message
    );
}

/// Gate F3 (R-TYP-28): a record-domain constructor in a check yields its underlying record and asserts no membership of
/// its own, though its fields are still held to theirs.
#[test]
fn gate_f3_a_record_domain_constructor_in_a_check_asserts_no_membership() {
    let text = "language: velme/0.2

type Even = Number where floor(value / 2) * 2 == value
type Pair where value.low <= value.high and value.low == value.high - 2:
    low: Number
    high: Number
type Holder where value.n == value.n:
    n: Even

goal Swap(p: Pair) -> Pair:
    plan: \"Keep it.\"
    check:
        - result != Pair(low: 3, high: 2)
        - Holder(n: 4).n == 4
";
    let program = program(text);
    assert_eq!(check_literals(&program, text), Vec::new());
    let odd = text.replace("Holder(n: 4)", "Holder(n: 3)");
    let program = velme_test_support::program(&odd);
    let diags = check_literals(&program, &odd);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert!(
        diags[0].message.starts_with("`3` isn\'t a valid `Even`"),
        "{}",
        diags[0].message
    );
}

/// Gate F1: whatever sema accepts lowers. For every accepted golden and example, every goal's checks and every
/// domain's condition lower and validate, so `velme check`\'s "valid" can\'t precede a VL0607 in `velme build`.
#[allow(clippy::disallowed_methods)] // test-only fixture reads, as in tests/lower.rs
#[test]
fn gate_f1_sema_accepted_programs_lower() {
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "velme") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect(&root.join("tests/golden/sema/accept"), &mut files);
    collect(&root.join("examples"), &mut files);
    files.sort();
    let mut lowered = 0;
    for path in files {
        let bytes = std::fs::read(&path).expect("readable");
        let file = velme_sema::SourceFile::from_bytes(path.display().to_string(), bytes).expect("UTF-8");
        let (program, diags) = velme_sema::analyze(&file);
        let Some(program) = program.filter(|_| !diags.iter().any(velme_diagnostics::Diagnostic::is_error)) else {
            continue;
        };
        let shown = path.display();
        Membership::new(&program, &file.text).unwrap_or_else(|d| panic!("{shown}: {d:?}"));
        for i in 0..program.goals.len() {
            GoalChecks::new(&program, velme_sema::hir::GoalId(i), &file.text)
                .unwrap_or_else(|d| panic!("{shown}: {d:?}"));
        }
        assert_eq!(
            check_literals(&program, &file.text)
                .iter()
                .filter(|d| d.is_error())
                .count(),
            0,
            "{shown}"
        );
        lowered += 1;
    }
    assert!(lowered >= 3, "the goldens and examples were found");
}

/// Gate R1 (R-TYP-31): a budget or size failure while testing a literal in a domain condition keeps its code, where
/// a non-member is `VL0209`.
#[test]
fn gate_r1_a_condition_literal_failure_keeps_its_code() {
    let text = "language: velme/0.2

type Slow = Number where range(10000000).length > value
type Holder:
    a: Slow
type Tagged = Number where Holder(a: 1).a == value
";
    let program = program(text);
    let diags = check_literals(&program, text);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].code, Code::SizeLimitExceeded, "{diags:#?}");
    let member = "language: velme/0.2

type Even = Number where floor(value / 2) * 2 == value
type Holder:
    a: Even
type Tagged = Number where Holder(a: 3).a == value
";
    let program = velme_test_support::program(member);
    let diags = check_literals(&program, member);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].code, Code::InvalidDomain);
}
