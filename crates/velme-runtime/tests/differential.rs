//! Leaf goals on both backends through the differential harness (`runtime/31` R-SBX-15, `delivery/51` §2-3, D-118):
//! the typed valid-IR generator's coverage and its property test, the fuzz corpus on stable, the WASM halves of the
//! built-in and number criteria, and the fuel per second of each backend.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use arbitrary::Unstructured;
use proptest::prelude::{ProptestConfig, any};
use proptest::test_runner::{RngSeed, TestCaseError};
use serde_json::{Value as Json, json};
use velme_builtins::execution::Limits;
use velme_builtins::limits::{MAX_FUEL, MAX_GOAL_CALLS, MAX_WALL_CLOCK_MS};
use velme_builtins::{BUILTINS_VERSION, CATALOG, Number, Value};
use velme_diagnostics::Code;
use velme_ir::{IR_VERSION, Node, ValidIr, decode_str, encode_value, to_canonical_string};
use velme_runtime::{Backend, Wasm, eval_leaf};
use velme_sema::hir::{GoalId, Program};
use velme_test_support::differential::{Outcome, differential};
use velme_test_support::generate::{check, generate};
use velme_test_support::ir_json::{binary, builtin, input};
use velme_test_support::{goal_id, program, read, repo, valid_ir};

/// One backend for every case, with no disk cache; it keeps the modules it compiled last in memory (R-SBX-20).
static WASM: LazyLock<Arc<Wasm>> = LazyLock::new(|| Arc::new(Wasm::new(None)));

/// `len` bytes from `seed`, the same on every run and platform (splitmix64).
fn entropy(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed;
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        bytes.extend((z ^ (z >> 31)).to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

// ---- the generator ----

/// Every goal the generator makes validates, and together they use every node kind a body may hold, every operator
/// and every value built-in (D-118): the test says so if the catalog grows past the generator.
#[test]
fn r_sbx_15_generated_goals_validate_and_cover_every_node_kind_operator_and_builtin() {
    let (mut kinds, mut operators, mut called) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for seed in 0..400 {
        let leaf = generate(&mut Unstructured::new(&entropy(seed, 1024))).leaf();
        for node in leaf.ir.goal().body.preorder() {
            let json = serde_json::to_value(node).expect("a node serializes");
            kinds.insert(json["kind"].as_str().expect("a kind").to_owned());
            match node {
                Node::BinaryOp { op, .. } => operators.insert(to_canonical_string(op).expect("an operator")),
                Node::UnaryOp { op, .. } => operators.insert(to_canonical_string(op).expect("an operator")),
                Node::Builtin { name, .. } => called.insert(name.clone()),
                _ => false,
            };
        }
    }
    // Every kind of `compiler/21` §3 but `call`, which no body may hold (R-IR-02).
    assert_eq!(kinds.len(), 19, "{kinds:?}");
    assert_eq!(operators.len(), 15, "{operators:?}");
    let catalog: BTreeSet<String> = CATALOG
        .iter()
        .filter(|b| b.function.is_some())
        .map(|b| b.name.to_owned())
        .collect();
    assert_eq!(called, catalog);
}

proptest::proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        rng_seed: RngSeed::Fixed(118),
        ..ProptestConfig::default()
    })]

    /// The cross-backend row of `delivery/51` §3 on generated leaves: the same value or full diagnostic, fuel and memory
    /// on both backends, within the system limits, one unit of fuel or byte of memory short, and within a fraction of
    /// each, with no run near a quarter of its Wasmtime backstop (AC-QA-05, R-SBX-15, D-118).
    #[test]
    fn ac_qa_05_generated_leaves_are_the_same_on_both_backends(bytes in proptest::collection::vec(any::<u8>(), 0..1536)) {
        let generated = generate(&mut Unstructured::new(&bytes));
        check(&WASM, &generated).map_err(TestCaseError::fail)?;
    }
}

/// A longer run of the same, for a gate or a change to either backend: `cargo test --release -p velme-runtime --test
/// differential -- --ignored many_generated`.
#[test]
#[ignore = "a soak run, some three minutes in release"]
fn r_sbx_15_many_generated_leaves_are_the_same_on_both_backends() {
    for seed in 0..20_000 {
        let generated = generate(&mut Unstructured::new(&entropy(seed, 1024)));
        check(&WASM, &generated).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
}

/// On stable, the committed corpus of the `differential` fuzz target runs through the target's logic, and the fuzz
/// smoke job runs every target of `fuzz/Cargo.toml`, each with a committed corpus (AC-QA-06, D-118). The `parse` and
/// `validate` corpora are replayed by `ac_syn_13_*` and `ac_ir_06_*`.
#[test]
fn ac_qa_06_the_differential_corpus_replays_and_ci_fuzzes_every_target() {
    let manifest: toml::Table = toml::from_str(&read(&repo("fuzz/Cargo.toml"))).expect("fuzz/Cargo.toml");
    let targets: BTreeSet<&str> = manifest["bin"]
        .as_array()
        .expect("fuzz targets")
        .iter()
        .map(|bin| bin["name"].as_str().expect("a name"))
        .collect();
    assert!(targets.contains("differential"), "{targets:?}");
    let workflow = read(&repo(".github/workflows/ci.yml"));
    let listed: BTreeSet<&str> = workflow
        .lines()
        .find_map(|line| line.trim().strip_prefix("FUZZ_TARGETS:"))
        .expect("the fuzz job lists its targets")
        .split_whitespace()
        .collect();
    assert_eq!(listed, targets, "the fuzz smoke job runs every target");
    for target in &targets {
        let corpus = std::fs::read_dir(repo(&format!("fuzz/corpus/{target}"))).expect("a committed corpus");
        assert!(corpus.count() > 0, "{target}");
    }
    let mut replayed = 0;
    for entry in std::fs::read_dir(repo("fuzz/corpus/differential")).expect("the corpus") {
        let path = entry.expect("an entry").path();
        let bytes = std::fs::read(&path).expect("a corpus file");
        let generated = generate(&mut Unstructured::new(&bytes));
        check(&WASM, &generated).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        replayed += 1;
    }
    assert!(replayed >= 8, "{replayed}");
}

// ---- the WASM halves (D-118) ----

/// The leaf `G(params) -> output` whose body is `body`, with its program and validated IR.
fn leaf(params: &[(&str, &str)], output: &str, body: Json) -> (Program, GoalId, ValidIr) {
    let signature: Vec<String> = params.iter().map(|(name, ty)| format!("{name}: {ty}")).collect();
    let source = format!(
        "language: velme/0.1\n\ngoal G({}) -> {output}:\n    plan: \"x\"\n",
        signature.join(", ")
    );
    let program = program(&source);
    let inputs: Vec<Json> = params.iter().map(|(name, ty)| json!([name, {"t": ty}])).collect();
    let document = json!({
        "ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "G", "types": {},
        "inputs": inputs, "output": {"t": output}, "body": body,
    });
    let (ir, goal) = (valid_ir(&program, &document.to_string()), goal_id(&program, "G"));
    (program, goal, ir)
}

/// `(program, goal, ir)` on JSON `inputs`, through the harness: what both backends gave, which it has checked is the
/// same, and the same one unit of fuel and one byte of memory short.
fn both((program, goal, ir): &(Program, GoalId, ValidIr), inputs: &[&str]) -> Outcome {
    let params = &program.goals[goal.0].params;
    let inputs: Vec<Value> = inputs
        .iter()
        .zip(params)
        .map(|(text, param)| decode_str(text, &param.ty, program).expect("decodes"))
        .collect();
    differential(&WASM, program, *goal, ir, &inputs).unwrap_or_else(|e| panic!("{inputs:?}: {e}"))
}

fn number(text: &str) -> Json {
    json!({"kind": "literal", "type": {"t": "Number"}, "value": serde_json::from_str::<Json>(text).expect("a number")})
}

fn num(text: &str) -> Value {
    Value::Number(Number::parse(text).expect("a number"))
}

/// `random` gives the `language/14` §3 reference vectors on WASM as on the interpreter (AC-BLT-01).
#[test]
fn ac_blt_01_random_reference_vectors_wasm() {
    let random = leaf(
        &[("seed", "Number"), ("index", "Number")],
        "Number",
        builtin("random", &[input("seed"), input("index")]),
    );
    for (seed, index, want) in [
        ("0", "0", "0.883310808213642685"),
        ("0", "1", "0.431527997048510052"),
        ("42", "0", "0.741564878771823401"),
        ("42", "7", "0.800631876713503438"),
        ("-1", "0", "0.893942920283184507"),
    ] {
        let (value, _) = both(&random, &[seed, index]);
        assert_eq!(value, Ok(num(want)), "random({seed}, {index})");
    }
}

/// `round` ties away from zero on WASM as on the interpreter (AC-BLT-04).
#[test]
fn ac_blt_04_round_ties_away_from_zero_wasm() {
    let round = leaf(&[("x", "Number")], "Number", builtin("round", &[input("x")]));
    for (x, want) in [
        ("2.5", "3"),
        ("-2.5", "-3"),
        ("2.4", "2"),
        ("-2.4", "-2"),
        ("0.5", "1"),
        ("7", "7"),
    ] {
        assert_eq!(both(&round, &[x]).0, Ok(num(want)), "round({x})");
    }
}

/// `1 / 0` is `VL0602` on WASM, with the interpreter's full diagnostic (AC-TYP-06).
#[test]
fn ac_typ_06_division_by_zero_is_vl0602_wasm() {
    let divide = leaf(&[("x", "Number")], "Number", binary("div", input("x"), number("0")));
    let failure = both(&divide, &["1"]).0.expect_err("no answer");
    assert_eq!(failure.code, Code::ArithmeticError);
    assert!(failure.message.contains("divide 1 by 0"), "{}", failure.message);
}

/// Exact decimals on WASM as on the interpreter: `0.1 + 0.2 == 0.3`, `to_text(2.50)` is `"2.5"`, `1 / 3` renders 28
/// threes, and the JSON input `0.1` comes out `0.1` (AC-TYP-15).
#[test]
fn ac_typ_15_exact_decimal_arithmetic_wasm() {
    let sum = binary("eq", binary("add", number("0.1"), number("0.2")), number("0.3"));
    assert_eq!(both(&leaf(&[], "Boolean", sum), &[]).0, Ok(Value::Boolean(true)));
    let text = builtin("to_text", &[number("2.50")]);
    assert_eq!(both(&leaf(&[], "Text", text), &[]).0, Ok(Value::text("2.5")));
    let third = both(&leaf(&[], "Number", binary("div", number("1"), number("3"))), &[]).0;
    let rendered = encode_value(&third.expect("a third")).expect("encodes");
    assert_eq!(rendered, "0.3333333333333333333333333333");
    let echoed = both(&leaf(&[("x", "Number")], "Number", input("x")), &["0.1"]).0;
    assert_eq!(encode_value(&echoed.expect("the input")).expect("encodes"), "0.1");
}

// ---- fuel per second (D-51, D-118) ----

/// AC-RUN-05's workload: a `reduce` over `range(10000)` whose function reduces over `range(10000)`, which runs out of
/// `max_fuel` long before it ends.
fn nested_reduce() -> Json {
    let range = builtin("range", &[number("10000")]);
    let inner = json!({"kind": "reduce", "list": range, "init": {"kind": "local", "name": "outer"},
        "fn": {"acc": "sum", "param": "j", "body": binary("add", json!({"kind": "local", "name": "sum"}), number("1"))}});
    json!({"kind": "reduce", "list": range, "init": number("0"), "fn": {"acc": "outer", "param": "i", "body": inner}})
}

/// The time of `run`, which must spend all of `max_fuel` and fail with `VL0601`: the best of three.
fn timed(mut run: impl FnMut() -> Outcome) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            let (result, spent) = run();
            let took = start.elapsed();
            assert_eq!(result.expect_err("out of fuel").code, Code::BudgetExceeded);
            assert_eq!(spent.fuel, MAX_FUEL);
            took
        })
        .min()
        .expect("three runs")
}

/// The fuel per second of each backend, printed for the gate report: the interpreter; WASM with its module compiled
/// in memory; and WASM with no disk cache, a new sandbox and a compile on every run. The deterministic bound of a
/// whole run is `max_goal_calls` × `max_fuel` (D-51); this fails only if that would take longer than
/// `max_wall_clock` on either backend, counting a compile for each call. The speed target is M8's (`delivery/51` §6).
#[test]
#[ignore = "a release-mode measurement: cargo test --release -p velme-runtime --test differential -- --ignored --nocapture fuel_per_second"]
fn fuel_per_second_keeps_the_largest_run_under_the_wall_clock() {
    if cfg!(debug_assertions) {
        println!("fuel_per_second measures only in release: add --release");
        return;
    }
    let (program, goal, ir) = leaf(&[], "Number", nested_reduce());
    let target = &program.goals[goal.0];
    let on = |backend: &Backend| eval_leaf(backend, target, &ir, Vec::new(), Limits::SYSTEM, None);
    let interp = timed(|| on(&Backend::Interp));
    let warm = Backend::Wasm(Arc::new(Wasm::new(None)));
    let _ = on(&warm);
    let wasm = timed(|| on(&warm));
    let cold = timed(|| on(&Backend::Wasm(Arc::new(Wasm::new(None)))));
    let limit = Duration::from_millis(MAX_WALL_CLOCK_MS);
    for (name, took) in [
        ("interp", interp),
        ("wasm, compiled", wasm),
        ("wasm, compiled each run", cold),
    ] {
        let per_second = MAX_FUEL as f64 / took.as_secs_f64();
        let largest = took * u32::try_from(MAX_GOAL_CALLS).expect("fits");
        println!("{name}: {per_second:.3e} fuel/s; {MAX_GOAL_CALLS} calls at max_fuel take {largest:.2?}");
        assert!(largest <= limit, "{name}: {largest:?} is over {limit:?}");
    }
}
