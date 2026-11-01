//! Run-time membership (`runtime/30` R-RUN-26–28): the requested goal's inputs and every goal's output are walked by
//! `velme-check`'s membership, called by the runtime around each body whichever backend runs it, on the invocation's
//! own fuel; and verification enters through the same walk (`compiler/22` §6 steps 8a, 8c).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value as Json, json};
use velme_builtins::execution::Limits;
use velme_builtins::{Number, Value};
use velme_diagnostics::Code;
use velme_interp::Spent;
use velme_ir::{Fingerprint, Origin, Request, decode_value, encode_value, to_canonical_string, validate, wired_goal};
use velme_runtime::{
    Backend, BuildInput, GoalRun, Lock, Mode, Options, Registry, Status, Store, Wasm, build, eval_leaf_after, run_goal,
    run_goal_unchecked, test_goal,
};
use velme_sema::hir::Program;
use velme_synth::{Scripted, SynthOptions};
use velme_test_support::ir_json::{binary, input, literal, local};
use velme_test_support::{goal_id, install, program};

const FILE: &str = "members.velme";

const SOURCE: &str = "language: velme/0.2

type Score = Number where value >= 0
type Lucky = Number where value != 13
type Ratio = Number where 10 / value > 1
type Loud = Text where concat(value, \"!\") != \"!\"
type Heavy = Number where every i in range(10000) has every j in range(20) has i + j >= value
type Pair where value.low <= value.high:
    low: Number
    high: Number
type Card:
    pair: Pair
    scores: List<Score?>
type Tagged:
    note: Text
    score: Score

goal Shift(n: Number) -> Score:
    plan: \"Take one off n.\"
    check:
        - result >= 100

goal Keep(s: Score) -> Number:
    plan: \"Divide s by zero.\"

goal Echo(l: Lucky) -> Number:
    plan: \"Return l.\"
    examples:
        - Echo(13) == 13

goal Maybe(s: Score?) -> Number:
    plan: \"Return one.\"

goal Divide(r: Ratio) -> Number:
    plan: \"Return r.\"

goal Shout(t: Loud) -> Loud:
    plan: \"Return t.\"

goal Busy(h: Heavy) -> Number:
    budget cpu=1ms
    plan: \"Return h.\"

goal Tag(t: Text, s: Number) -> Tagged:
    plan: \"Tag t with s.\"
    check:
        - result.score <= 10

goal Bounds(n: Number) -> Pair:
    plan: \"From n down to zero.\"

goal Deal(n: Number) -> Card:
    plan: \"A card of n.\"

goal Chain(n: Number) -> Number:
    call:
        a = Shift(n)
        result = Keep(a)

goal Outer(n: Number) -> Score:
    call:
        a = Shift(n)
    plan: \"Take a thousand off a.\"
    check:
        - result >= 100
";

fn n(x: i64) -> Value {
    Value::from(Number::from(x))
}

/// The hand-written body of goal `name`.
fn body(name: &str) -> Json {
    let pair = |low: Json, high: Json| json!({"kind": "record", "type": "Pair", "fields": {"low": low, "high": high}});
    match name {
        "Shift" => binary("sub", input("n"), literal(1)),
        "Keep" => binary("div", input("s"), literal(0)),
        "Echo" => input("l"),
        "Maybe" | "Busy" => literal(1),
        "Divide" => input("r"),
        "Shout" => input("t"),
        "Bounds" => pair(input("n"), literal(0)),
        "Tag" => json!({"kind": "record", "type": "Tagged", "fields": {"note": input("t"), "score": input("s")}}),
        "Deal" => json!({"kind": "record", "type": "Card", "fields": {
            "pair": pair(literal(-100), input("n")),
            "scores": {"kind": "list", "of": {"t": "Optional", "of": {"t": "Number"}},
                       "items": [{"kind": "literal", "type": {"t": "Nothing"}, "value": null}, input("n")]}}}),
        "Chain" => local("result"),
        "Outer" => binary("sub", local("a"), literal(1000)),
        _ => panic!("no goal {name}"),
    }
}

/// The IR of goal `name`: the compiler's tables, signature and calls, with [`body`].
fn ir(program: &Program, name: &str) -> String {
    let goal = wired_goal(program, goal_id(program, name)).expect("the compiler's half");
    let mut ir = serde_json::to_value(goal).expect("IR");
    ir["body"] = body(name);
    ir.to_string()
}

/// A project with every goal installed, and its checked program.
fn installed(name: &str) -> (PathBuf, Program) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("membership").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    let program = program(SOURCE);
    for goal in &program.goals {
        install(&dir, FILE, &program, &ir(&program, &goal.name));
    }
    (dir, program)
}

fn registry(project: &Path, program: &Program, goal: &str) -> Registry {
    let lock = Lock::read(project).expect("lock").expect("a lock");
    Registry::load(program, goal_id(program, goal), FILE, &lock, &Store::new(project)).expect("registry")
}

fn run_on(project: &Path, program: &Program, goal: &str, inputs: Vec<Value>, backend: Backend) -> GoalRun {
    let options = Options {
        backend,
        ..Options::default()
    };
    let id = goal_id(program, goal);
    run_goal(program, id, SOURCE, &registry(project, program, goal), inputs, options)
}

fn run(project: &Path, program: &Program, goal: &str, inputs: Vec<Value>) -> GoalRun {
    run_on(project, program, goal, inputs, Backend::Interp)
}

/// The run's diagnostics' codes and messages.
fn failure(run: &GoalRun) -> Vec<(Code, String)> {
    run.result()
        .expect_err("a failure")
        .into_iter()
        .map(|d| (d.code, d.message))
        .collect()
}

/// AC-RUN-13, AC-RUN-14, AC-IR-21: every leaf and composite output is walked before its own checks: a widened computed
/// value that validated passes no further, a cross-field record and a nested list item report their first path, and
/// `nothing` passes; a failed child propagates, and no own check runs after the failure.
#[test]
fn ac_run_13_every_output_is_walked_before_its_checks() {
    let (project, program) = installed("ac_run_13");
    let shift = run(&project, &program, "Shift", vec![n(0)]);
    assert_eq!(
        failure(&shift),
        [(
            Code::OutputOutsideDomain,
            "`Shift` returned a value outside `Score` at `result`: it must be at least 0.".to_owned()
        )]
    );
    assert!(shift.checks.is_empty(), "no own check ran: {:?}", shift.checks);
    // A member still meets its check.
    let checked = run(&project, &program, "Shift", vec![n(5)]);
    assert_eq!(failure(&checked)[0].0, Code::CheckFailed);
    assert_eq!(run(&project, &program, "Shift", vec![n(101)]).result(), Ok(n(100)));

    let bounds = run(&project, &program, "Bounds", vec![n(5)]);
    assert_eq!(
        failure(&bounds),
        [(
            Code::OutputOutsideDomain,
            "`Bounds` returned a value outside `Pair` at `result`: `value.low <= value.high` is false.".to_owned()
        )]
    );
    assert!(run(&project, &program, "Bounds", vec![n(-5)]).result().is_ok());

    let deal = run(&project, &program, "Deal", vec![n(-1)]);
    assert_eq!(
        failure(&deal),
        [(
            Code::OutputOutsideDomain,
            "`Deal` returned a value outside `Score` at `result.scores[1]`: it must be at least 0.".to_owned()
        )]
    );
    assert!(run(&project, &program, "Deal", vec![n(3)]).result().is_ok());
    // Depth first in declaration order: `pair` comes before `scores`.
    let deal = run(&project, &program, "Deal", vec![n(-200)]);
    assert!(
        failure(&deal)[0].1.contains("outside `Pair` at `result.pair`"),
        "{deal:?}"
    );

    // The composite's child fails first and propagates; on a member child, the composite's own output fails.
    let outer = run(&project, &program, "Outer", vec![n(0)]);
    assert_eq!(failure(&outer)[0].0, Code::OutputOutsideDomain);
    assert!(
        failure(&outer)[0]
            .1
            .starts_with("`Outer` failed because `Shift` failed: `Shift` returned")
    );
    let outer = run(&project, &program, "Outer", vec![n(500)]);
    assert_eq!(
        failure(&outer),
        [(
            Code::OutputOutsideDomain,
            "`Outer` returned a value outside `Score` at `result`: it must be at least 0.".to_owned()
        )]
    );
    assert!(outer.checks.is_empty());
    // A wired goal's child output too, before the next wave.
    let chain = run(&project, &program, "Chain", vec![n(0)]);
    assert_eq!(failure(&chain)[0].0, Code::OutputOutsideDomain);
}

/// Review finding D4-4: the output size is enforced last (`runtime/30` §4 steps 7–9): an oversized answer outside its
/// domain is `VL0504`, an oversized member whose check fails is `VL0501`, and only an oversized answer that passes both
/// is `VL0606`, on either backend.
#[test]
fn r_run_26_membership_and_checks_come_before_the_output_size() {
    let (project, program) = installed("output_size_last");
    let long = Value::text(&"a".repeat(1_100_000));
    let cases = [
        (n(-1), Code::OutputOutsideDomain),
        (n(50), Code::CheckFailed),
        (n(5), Code::SizeLimitExceeded),
    ];
    for backend in [Backend::Interp, Backend::Wasm(Arc::new(Wasm::new(None)))] {
        for (score, code) in &cases {
            let ran = run_on(
                &project,
                &program,
                "Tag",
                vec![long.clone(), score.clone()],
                backend.clone(),
            );
            assert_eq!(failure(&ran)[0].0, *code, "{score:?} on {backend:?}");
        }
        let short = run_on(&project, &program, "Tag", vec![Value::text("a"), n(5)], backend.clone());
        assert!(short.result().is_ok(), "{short:?}");
    }
}

/// AC-RUN-16, AC-ERR-05: invalid inputs are `VL0902` before the body runs, a condition's value error too, with its
/// path, domain and cause, while members and `nothing` pass; a predicate out of budget keeps `VL0601`.
#[test]
fn ac_run_16_an_invalid_input_is_vl0902_before_the_body() {
    let (project, program) = installed("ac_run_16");
    // `Keep`'s body divides by zero: `VL0902` means it never ran.
    let keep = run(&project, &program, "Keep", vec![n(-1)]);
    let diags = keep.result().expect_err("refused");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, Code::InvalidInput);
    assert_eq!(diags[0].message, "Input `s` is outside `Score`: it must be at least 0.");
    assert_eq!(
        failure(&run(&project, &program, "Keep", vec![n(1)]))[0].0,
        Code::ArithmeticError
    );

    let divide = run(&project, &program, "Divide", vec![n(0)]);
    let diags = divide.result().expect_err("refused");
    assert_eq!(diags[0].code, Code::InvalidInput);
    assert_eq!(
        diags[0].message,
        "Input `r` is outside `Ratio`: `10 / value > 1` tried to divide 10 by 0, which has no answer."
    );
    assert_eq!(diags[0].notes, ["cause: VL0602"]);

    assert_eq!(
        run(&project, &program, "Maybe", vec![Value::Nothing]).result(),
        Ok(n(1))
    );
    assert_eq!(
        failure(&run(&project, &program, "Maybe", vec![n(-3)]))[0].0,
        Code::InvalidInput
    );
    // 10 000 × 20 items is past `cpu=1ms`'s 100 000 fuel.
    assert_eq!(
        failure(&run(&project, &program, "Busy", vec![n(0)]))[0].0,
        Code::BudgetExceeded
    );
}

/// AC-RUN-16, AC-SYNTH-69: verification and examples enter through the same walk, where a non-member is a bug of
/// ours (`VL0607`), not `VL0902`.
#[test]
fn ac_run_16_a_verification_input_outside_its_domain_is_vl0607() {
    let (project, program) = installed("ac_run_16_verification");
    let id = goal_id(&program, "Echo");
    let registry = registry(&project, &program, "Echo");
    let entry = run_goal_unchecked(&program, id, SOURCE, &registry, vec![n(13)], Options::default());
    assert_eq!(failure(&entry)[0].0, Code::InternalError);
    let tested = test_goal(&program, id, SOURCE, &registry, &Options::default()).expect_err("refused");
    assert_eq!(tested[0].code, Code::InternalError, "{tested:#?}");
    let busy = goal_id(&program, "Busy");
    let registry = self::registry(&project, &program, "Busy");
    let entry = run_goal_unchecked(&program, busy, SOURCE, &registry, vec![n(0)], Options::default());
    assert_eq!(
        failure(&entry)[0].0,
        Code::BudgetExceeded,
        "a budget failure keeps its code"
    );
}

/// AC-RUN-15, AC-RUN-17: the interpreter and WASM give the same values, paths, fuel and memory, the input walk's fuel
/// spent before the body and the output walk's after it.
#[test]
fn ac_run_17_both_backends_agree_on_membership() {
    let (project, program) = installed("ac_run_17");
    let wasm = Backend::Wasm(Arc::new(Wasm::new(None)));
    let cases: [(&str, Value); 9] = [
        ("Shout", Value::text("hey")),
        ("Shout", Value::text("")),
        ("Keep", n(-1)),
        ("Divide", n(0)),
        ("Divide", n(5)),
        ("Shift", n(0)),
        ("Shift", n(101)),
        ("Bounds", n(5)),
        ("Deal", n(-1)),
    ];
    for (goal, value) in cases {
        let on_interp = run_on(&project, &program, goal, vec![value.clone()], Backend::Interp);
        let on_wasm = run_on(&project, &program, goal, vec![value.clone()], wasm.clone());
        assert_eq!(on_interp.result(), on_wasm.result(), "{goal}({value:?})");
        assert_eq!(
            (on_interp.fuel, on_interp.memory),
            (on_wasm.fuel, on_wasm.memory),
            "{goal}({value:?})"
        );
    }
    // The condition's `concat` allocates, on the invocation's memory, on both backends alike.
    let shout = run_on(&project, &program, "Shout", vec![Value::text("hey")], wasm.clone());
    assert!(shout.memory > 0, "{shout:?}");
    // `Divide(5)`: its input walk (a domain, its base and `10 / value > 1`, five nodes), its body (one node) and its
    // Number output, which has nothing to walk.
    let divide = run_on(&project, &program, "Divide", vec![n(5)], wasm);
    assert_eq!(divide.fuel, 2 + 5 + 1);
}

/// AC-RUN-17: a body gets only the fuel its input walk left, on both backends: with one unit less it runs out and names
/// the invocation's own limit.
#[test]
fn ac_run_17_the_body_gets_only_what_the_input_walk_left() {
    let program = program(SOURCE);
    let id = goal_id(&program, "Shift");
    let request = Request {
        program: &program,
        goal: id,
        calls: &[],
        origin: Origin::Complete,
    };
    let valid = validate(&ir(&program, "Shift"), &request).expect("valid IR");
    let goal = &program.goals[id.0];
    let limits = Limits {
        fuel: 1_000,
        memory: Limits::SYSTEM.memory,
    };
    let wasm = Backend::Wasm(Arc::new(Wasm::new(None)));
    for backend in [Backend::Interp, wasm] {
        let body = |spent: u64| {
            let spent = Spent { fuel: spent, memory: 0 };
            eval_leaf_after(&backend, goal, &valid, vec![n(4)], limits, spent, None)
        };
        let (fresh, used) = body(0);
        assert_eq!(fresh, Ok(n(3)));
        let cost = used.fuel;
        let (value, spent) = body(limits.fuel - cost);
        assert_eq!((value, spent.fuel), (Ok(n(3)), limits.fuel), "{backend:?}");
        let (value, spent) = body(limits.fuel - cost + 1);
        let diagnostic = value.expect_err("out of fuel");
        assert_eq!(diagnostic.code, Code::BudgetExceeded, "{backend:?}");
        assert_eq!(diagnostic.notes, ["it may take 1000 steps (fuel)"], "{backend:?}");
        assert_eq!(spent.fuel, limits.fuel, "{backend:?}");
    }
}

/// AC-TYP-28: a domain value's JSON and canonical hash are its base's, with no domain tag, nested and `nothing` too.
#[test]
fn ac_typ_28_domain_values_encode_and_hash_as_their_bases() {
    let (project, program) = installed("ac_typ_28");
    let card = run(&project, &program, "Deal", vec![n(3)]).result().expect("a member");
    let base = Value::record(
        "Card",
        vec![
            (
                "pair".to_owned(),
                Value::record("Pair", vec![("low".to_owned(), n(-100)), ("high".to_owned(), n(3))]),
            ),
            ("scores".to_owned(), Value::list(vec![Value::Nothing, n(3)])),
        ],
    );
    assert_eq!(card, base);
    let json = encode_value(&card).expect("JSON");
    assert_eq!(json, r#"{"pair":{"low":-100,"high":3},"scores":[null,3]}"#);
    assert!(!json.contains("Domain") && !json.contains("Pair"));
    let parsed: Json = serde_json::from_str(&json).expect("JSON");
    let canonical = to_canonical_string(&parsed).expect("canonical");
    assert_eq!(canonical, r#"{"pair":{"high":3,"low":-100},"scores":[null,3]}"#);
    let base_json: Json = serde_json::from_str(&encode_value(&base).expect("JSON")).expect("JSON");
    assert_eq!(Fingerprint::of(&parsed).ok(), Fingerprint::of(&base_json).ok());
    // The same JSON decodes at the domain-typed output as its base value.
    let output = &program.goals[goal_id(&program, "Deal").0].output;
    assert_eq!(decode_value(&parsed, output, &program).ok(), Some(base));
}

/// The program [`built`] builds: `Echo`'s verification walks its inputs, `Pick`'s candidate returns a non-member.
const BUILT: &str = "language: velme/0.2

type Score = Number where value >= 0

goal Echo(s: Score) -> Number:
    plan: \"Return s.\"
    check:
        - result >= 0
    examples:
        - Echo(4) == 4

goal Pick(n: Number) -> Score:
    plan: \"Return n minus one.\"
    examples:
        - Pick(0) == 5
";

/// AC-SYNTH-69: verification enters through the runtime's input walk and counts its fuel in `max_fuel_observed`, which
/// is what `run` takes on the same input; an output outside its domain is `VL0504`, wrapped as `VL0503`, before the
/// example is compared.
#[test]
fn ac_synth_69_verification_walks_inputs_and_outputs_as_run_does() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("membership")
        .join("ac_synth_69");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    let program = program(BUILT);
    let replies = [
        json!({"body": input("s")}),
        json!({"body": binary("sub", input("n"), literal(1))}),
    ];
    let backend = Scripted::replies(replies.iter().map(Json::to_string));
    let input = BuildInput {
        mode: Mode::Build,
        program: &program,
        source: BUILT,
        project: &dir,
        file: FILE,
        backend: Some(&backend),
        options: SynthOptions {
            max_retries: 0,
            ..SynthOptions::default()
        },
        run: Options::default(),
    };
    let report = build(&input, &mut || {});
    let echo = &report.goals[0];
    assert!(matches!(echo.status, Status::Built(_)), "{report:#?}");
    let pick = &report.goals[1];
    assert_eq!(pick.status, Status::Failed, "{report:#?}");
    // The example `Pick(0) == 5` gets `-1`: the walk refuses it before the example is compared.
    let attempts = pick.attempts.join("\n");
    assert!(
        attempts.starts_with(
            "attempt 1: VL0503: running it when `n` is `0` failed: `Pick` returned a value outside `Score`"
        ),
        "{attempts}"
    );
    let log = fs::read_to_string(dir.join(".velme/synth-log.jsonl")).expect("synth log");
    let line: Json = log
        .lines()
        .map(|l| serde_json::from_str::<Json>(l).expect("a log line"))
        .find(|l| l["goal"] == "Pick")
        .expect("Pick's attempt");
    assert_eq!(
        (&line["outcome"], &line["cause"]),
        (&json!("VL0504"), &json!("runtime-VL0504"))
    );

    let lock = Lock::read(&dir).expect("lock").expect("present");
    let entry = lock.entries().iter().find(|e| e.name == "Echo").expect("Echo locked");
    let artifact = Store::new(&dir).get(entry.artifact).expect("artifact");
    let id = goal_id(&program, "Echo");
    let registry = Registry::load(&program, id, FILE, &lock, &Store::new(&dir)).expect("registry");
    let ran = run_goal(&program, id, BUILT, &registry, vec![n(4)], Options::default());
    assert_eq!(ran.result(), Ok(n(4)));
    // The input walk (2), `value >= 0` (3), the body (1) and the check (3), on every input alike.
    assert_eq!(ran.fuel, 9);
    assert_eq!(artifact.manifest.verification.max_fuel_observed, ran.fuel);
}
