//! The `delivery/51` §6 targets of the runtime (D-130, D-131): the nested `reduce` on the interpreter and on WASM, and
//! the scheduler's cost per call. Ignored; `cargo xtask gate` runs them in release.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use velme_builtins::execution::Limits;
use velme_builtins::limits::MAX_GOAL_CALLS;
use velme_builtins::{BUILTINS_VERSION, Number, Value};
use velme_ir::{IR_VERSION, ValidIr};
use velme_runtime::{Backend, Lock, Options, Registry, Store, Wasm, eval_leaf, run_goal};
use velme_sema::hir::Goal;
use velme_test_support::workload::{
    FAN_FILE, assert_release, best_of, fan_ir, fan_out, fan_source, nested_reduce, paired_ratios,
};
use velme_test_support::{goal_id, program, valid_ir};

/// `G() -> Number` whose body is the nested `reduce` over `range(1000)`, and its IR.
fn reduce_goal() -> (Goal, ValidIr) {
    let program = program("language: velme/0.1\n\ngoal G() -> Number:\n    plan: \"Count to a million.\"\n");
    let document = json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "G", "types": {},
        "inputs": [], "output": {"t": "Number"}, "body": nested_reduce(1000)});
    let ir = valid_ir(&program, &document.to_string());
    (program.goals[0].clone(), ir)
}

/// One run of the nested `reduce` on `backend`, giving a million.
fn reduce_once(backend: &Backend, goal: &Goal, ir: &ValidIr) {
    let (value, _) = eval_leaf(backend, goal, ir, Vec::new(), Limits::SYSTEM, None);
    assert_eq!(value.expect("a million"), Value::from(Number::from(1_000_000_i64)));
}

/// The best of five runs of the nested `reduce` on `backend`.
fn reduce_on(backend: &Backend) -> Duration {
    let (goal, ir) = reduce_goal();
    best_of(5, || reduce_once(backend, &goal, &ir))
}

/// The nested `reduce` over `range(1000)` takes under 200 ms on the interpreter (AC-QA-07, `delivery/51` §6, D-131).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_the_interpreter_runs_a_million_additions_under_200_ms() {
    assert_release("ac_qa_07");
    let took = reduce_on(&Backend::Interp);
    println!("nested reduce, interp: {took:.2?}");
    assert!(took < Duration::from_millis(200), "{took:?}");
}

/// The same workload on WASM, its module already compiled but instantiation and the run thread included, takes at
/// most half the interpreter's time, by the median ratio of 15 interleaved pairs (AC-QA-07, `delivery/51` §6, D-118,
/// D-130). A target until M8c has been measured (D-125). `cargo xtask gate` runs it alone in its process.
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_wasm_runs_the_same_workload_in_at_most_half_the_interpreters_time() {
    assert_release("ac_qa_07");
    let (goal, ir) = reduce_goal();
    // The warm-up runs compile the module this backend then keeps (R-SBX-20).
    let wasm = Backend::Wasm(Arc::new(Wasm::new(None)));
    let ratios = paired_ratios(
        15,
        || reduce_once(&Backend::Interp, &goal, &ir),
        || reduce_once(&wasm, &goal, &ir),
    );
    let (min, median, max) = (ratios[0], ratios[ratios.len() / 2], ratios[ratios.len() - 1]);
    println!(
        "nested reduce, wasm / interp over {} pairs: min {min:.2}, median {median:.2}, max {max:.2}",
        ratios.len()
    );
    assert!(
        median <= 0.5,
        "wasm takes {median:.2}× the interpreter's time, the median of {} pairs",
        ratios.len()
    );
}

/// The scheduler's cost per call: a run of `Fan`, 128 goals counting itself, less 128 evaluations of the trivial leaf
/// `Id` alone, divided by 128, is under 20 µs (AC-QA-07, `delivery/51` §6, D-131). One job, so the tree runs its leaves
/// one at a time, as the leaves alone are.
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_scheduler_overhead_per_call_is_under_20_us() {
    assert_release("ac_qa_07");
    let (dir, program) = fan_out(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf").join("fan"));
    let (source, fan, id) = (fan_source(), goal_id(&program, "Fan"), goal_id(&program, "Id"));
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let registry = Registry::load(&program, fan, FAN_FILE, &lock, &Store::new(&dir)).expect("registry");
    let one = || vec![Value::from(Number::from(1_i64))];
    let tree = best_of(20, || {
        let run = run_goal(
            &program,
            fan,
            &source,
            &registry,
            one(),
            Options::default().with_jobs(1),
        );
        assert_eq!(run.result(), Ok(Value::from(Number::from(1_i64))));
    });
    let (leaf, ir) = (&program.goals[id.0], valid_ir(&program, &fan_ir(&program, "Id")));
    let leaves = best_of(20, || {
        for _ in 0..MAX_GOAL_CALLS {
            let (value, _) = eval_leaf(&Backend::Interp, leaf, &ir, one(), Limits::SYSTEM, None);
            assert!(value.is_ok());
        }
    });
    let calls = u32::try_from(MAX_GOAL_CALLS).expect("fits");
    let per_call = tree.saturating_sub(leaves) / calls;
    println!("scheduler: {calls} calls {tree:.2?}, their leaves {leaves:.2?}, overhead per call {per_call:.2?}");
    assert!(per_call < Duration::from_micros(20), "{per_call:?}");
}
