//! The wave scheduler (`runtime/30` §4, §5) on hand-written IR: waves, concurrency, and D-9's source-order failures,
//! whatever `--jobs` is.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use velme_builtins::{BUILTINS_VERSION, Number, Value};
use velme_diagnostics::Code;
use velme_ir::{IR_VERSION, calls};
use velme_runtime::{CallStatus, GoalRun, Lock, Options, Registry, Store, run_goal, run_goal_peak};
use velme_sema::hir::Program;
use velme_test_support::{goal_id, install, program};

const FILE: &str = "sched.velme";

/// `Spin` counts `n * n` by reducing over `range(n)` inside a reduce over `range(n)`, so it takes time in proportion to `n * n`; `SlowBoom` and
/// `FastBoom` count the same way, then divide by zero. `Split` calls them together (wave 1), then `Spin` on the result
/// of the last one (wave 2); `Fan` calls `Spin` three times at once; `Main` is `runtime/30` §4's wired example.
const SOURCE: &str = "language: velme/0.1

goal Spin(n: Number) -> Number:
    plan: \"Count up to n.\"

goal SlowBoom(n: Number) -> Number:
    plan: \"Count up to n, then divide by zero.\"

goal FastBoom(n: Number) -> Number:
    plan: \"Count up to n, then divide by zero.\"

goal Double(x: Number) -> Number:
    plan: \"Multiply x by 2.\"

goal AddOne(x: Number) -> Number:
    plan: \"Add 1 to x.\"

goal Main(x: Number) -> Number:
    call:
        doubled = Double(x)
        result = AddOne(doubled)
    plan: \"Double x, then add one.\"

goal Fan(n: Number) -> Number:
    call:
        a = Spin(n)
        b = Spin(n)
        c = Spin(n)
    plan: \"Add the three counts.\"

goal Checked(n: Number) -> Number:
    plan: \"Return n.\"
    check:
        - result > n
        - result < n

goal Wrap(n: Number) -> Number:
    call:
        c = Checked(n)
    plan: \"Return the checked n.\"

goal Outer(n: Number) -> Number:
    call:
        a = FastBoom(n)
        b = Split(n, n)
    plan: \"Add them.\"

goal Split(slow: Number, fast: Number) -> Number:
    call:
        first = SlowBoom(slow)
        second = FastBoom(fast)
        third = Spin(fast)
        after = Spin(third)
    plan: \"Add the counts.\"
";

fn number() -> Json {
    json!({"t": "Number"})
}

fn input(name: &str) -> Json {
    json!({"kind": "input", "name": name})
}

fn local(name: &str) -> Json {
    json!({"kind": "local", "name": name})
}

fn literal(value: i64) -> Json {
    json!({"kind": "literal", "type": number(), "value": value})
}

fn binary(op: &str, left: Json, right: Json) -> Json {
    json!({"kind": "binary", "op": op, "left": left, "right": right})
}

/// `n * n`, counted up by one per element of `range(n)` inside each element of `range(n)`.
fn count(n: &str) -> Json {
    let range = json!({"kind": "builtin", "name": "range", "args": [input(n)]});
    let inner = json!({"kind": "reduce", "list": range, "init": local("outer"),
                       "fn": {"acc": "sum", "param": "j", "body": binary("add", local("sum"), literal(1))}});
    json!({"kind": "reduce", "list": range, "init": literal(0),
           "fn": {"acc": "outer", "param": "i", "body": inner}})
}

/// The hand-written IR of goal `name` of [`SOURCE`], with the compiler's calls.
fn ir(program: &Program, name: &str) -> String {
    let (inputs, body) = match name {
        "Spin" => (json!([["n", number()]]), count("n")),
        "SlowBoom" | "FastBoom" => (json!([["n", number()]]), binary("div", count("n"), literal(0))),
        "Double" => (json!([["x", number()]]), binary("mul", input("x"), literal(2))),
        "AddOne" => (json!([["x", number()]]), binary("add", input("x"), literal(1))),
        "Main" => (json!([["x", number()]]), local("result")),
        "Fan" => (
            json!([["n", number()]]),
            binary("add", binary("add", local("a"), local("b")), local("c")),
        ),
        "Checked" => (json!([["n", number()]]), input("n")),
        "Wrap" => (json!([["n", number()]]), local("c")),
        "Outer" => (json!([["n", number()]]), local("b")),
        "Split" => (
            json!([["slow", number()], ["fast", number()]]),
            binary("add", local("third"), local("after")),
        ),
        _ => panic!("no goal {name}"),
    };
    let calls = calls(program, goal_id(program, name)).expect("calls");
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": name, "types": {},
           "inputs": inputs, "output": number(), "calls": calls, "body": body})
    .to_string()
}

/// A project with every goal of [`SOURCE`] installed, and its checked program.
fn installed(name: &str) -> (PathBuf, Program) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sched").join(name);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("project directory");
    let program = program(SOURCE);
    for goal in &program.goals {
        install(&dir, FILE, &program, &ir(&program, &goal.name));
    }
    (dir, program)
}

/// Runs `goal` of the program on `inputs` with `jobs` workers.
fn run(project: &Path, program: &Program, goal: &str, inputs: &[i64], jobs: usize) -> GoalRun {
    let id = goal_id(program, goal);
    let lock = Lock::read(project).expect("lock").expect("a lock");
    let registry = Registry::load(program, id, FILE, &lock, &Store::new(project)).expect("registry");
    let inputs = inputs.iter().map(|n| Value::from(Number::from(*n))).collect();
    run_goal(program, id, SOURCE, &registry, inputs, Options { jobs })
}

/// [`run`], and the most bodies evaluated at once.
fn run_peak(project: &Path, program: &Program, goal: &str, inputs: &[i64], jobs: usize) -> (GoalRun, usize) {
    let id = goal_id(program, goal);
    let lock = Lock::read(project).expect("lock").expect("a lock");
    let registry = Registry::load(program, id, FILE, &lock, &Store::new(project)).expect("registry");
    let inputs = inputs.iter().map(|n| Value::from(Number::from(*n))).collect();
    run_goal_peak(program, id, SOURCE, &registry, inputs, Options { jobs })
}

fn number_value(n: i64) -> Value {
    Value::from(Number::from(n))
}

/// Every call of `run`, as `binding:status`.
fn statuses(run: &GoalRun) -> Vec<String> {
    run.calls
        .iter()
        .map(|c| {
            let status = match c.status() {
                CallStatus::Ok => "ok",
                CallStatus::Failed => "failed",
                CallStatus::Skipped => "skipped",
            };
            format!("{}:{status}", c.binding)
        })
        .collect()
}

/// A number from a seeded sequence, so a test's "random" delays are the same on every run.
fn next(seed: &mut u64) -> i64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    i64::try_from(*seed >> 33).expect("fits") % 250
}

/// Three independent children of equal weight are all in wave 1 and three bodies are in flight at once with `--jobs 3`;
/// with one worker never more than one (AC-RUN-01, AC-RDM-03).
#[test]
fn ac_run_01_independent_children_overlap_in_time() {
    let (project, program) = installed("ac_run_01");
    let waves: Vec<usize> = run(&project, &program, "Fan", &[1], 1)
        .calls
        .iter()
        .map(|c| c.wave)
        .collect();
    assert_eq!(waves, [1, 1, 1]);
    // Heavy enough that three bodies starting together are all in flight at once, in a debug build too.
    let (parallel, peak) = run_peak(&project, &program, "Fan", &[400], 3);
    assert_eq!(parallel.result(), Ok(number_value(480_000)));
    assert_eq!(peak, 3);
    let (serial, peak) = run_peak(&project, &program, "Fan", &[400], 1);
    assert_eq!(peak, 1);
    assert_eq!(parallel, serial);
}

/// `Main` runs `Double` then `AddOne` in two waves and returns `2x + 1` (AC-RUN-02, AC-RDM-02).
#[test]
fn ac_run_02_a_wired_goal_runs_in_two_waves() {
    let (project, program) = installed("ac_run_02");
    for jobs in [1, 4] {
        let main = run(&project, &program, "Main", &[4], jobs);
        assert_eq!(main.outcome.as_ref().ok(), Some(&number_value(9)));
        let calls: Vec<(&str, &str, usize)> = main
            .calls
            .iter()
            .map(|c| (c.binding.as_str(), c.callee.as_str(), c.wave))
            .collect();
        assert_eq!(calls, [("doubled", "Double", 1), ("result", "AddOne", 2)]);
        assert_eq!(statuses(&main), ["doubled:ok", "result:ok"]);
        let value = |i: usize| main.calls[i].run.as_ref().expect("ran").outcome.as_ref().ok().cloned();
        assert_eq!((value(0), value(1)), (Some(number_value(8)), Some(number_value(9))));
    }
}

/// The failure of a run of `Split`, which needs `slow` and `fast` counts to fail.
fn split(project: &Path, program: &Program, slow: i64, fast: i64, jobs: usize) -> GoalRun {
    run(project, program, "Split", &[slow, fast], jobs)
}

/// Two siblings fail: the lower source-order one is reported however long each takes, both real outcomes are recorded,
/// the third sibling's success too, and the binding of the wave that never started is `skipped` (AC-RUN-03, D-9).
#[test]
fn ac_run_03_the_lowest_source_order_failure_is_reported() {
    let (project, program) = installed("ac_run_03");
    let mut seed = 7;
    for round in 0..100 {
        let (slow, fast) = (next(&mut seed), next(&mut seed));
        let run = split(&project, &program, slow, fast, 1 + round % 8);
        assert_eq!(
            statuses(&run),
            ["first:failed", "second:failed", "third:ok", "after:skipped"],
            "{round}"
        );
        let failed = run.outcome.as_ref().expect_err("Split fails");
        let reported = failed.diagnostics();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].code, Code::ArithmeticError);
        assert_eq!(
            reported[0].message,
            format!(
                "`Split` failed because `SlowBoom` failed: `SlowBoom` tried to divide {} by 0, which has no answer.",
                slow * slow
            )
        );
        assert_eq!(
            reported[0].notes,
            [format!(
                "`Split` also failed because `FastBoom` failed: `FastBoom` tried to divide {} by 0, which has no answer. [VL0602]",
                fast * fast
            )]
        );
        // The failing siblings' own outcomes are real ones, not cancellations.
        for call in &run.calls[..2] {
            let child = call.run.as_ref().expect("it ran");
            assert_eq!(
                child.outcome.as_ref().expect_err("it failed").code(),
                Some(Code::ArithmeticError)
            );
        }
        assert!(run.calls[3].run.is_none());
    }
}

/// The result, the failure with its notes, and every call's outcome are the same for `--jobs 1` and `--jobs 8`,
/// including two failing siblings where the slower one is first in source order (AC-RUN-04, D-9). What a trace holds
/// beyond timings is a [`GoalRun`], whose equality ignores them; the trace comparison itself follows the trace model.
#[test]
fn ac_run_04_the_run_is_the_same_for_every_job_count() {
    let (project, program) = installed("ac_run_04");
    let cases: [(&str, &[i64]); 5] = [
        ("Main", &[4]),
        ("Fan", &[100]),
        ("Split", &[250, 0]),
        ("Split", &[0, 250]),
        ("Split", &[100, 100]),
    ];
    for (goal, inputs) in cases {
        let one = run(&project, &program, goal, inputs, 1);
        for jobs in [2, 8] {
            let many = run(&project, &program, goal, inputs, jobs);
            assert_eq!(one, many, "{goal} {inputs:?} with {jobs} jobs");
            assert_eq!(one.result(), many.result());
        }
    }
    // The slower failure is the first in source order and is the one reported, with one job or eight.
    let slow_first = run(&project, &program, "Split", &[250, 0], 8);
    let message = slow_first.result().expect_err("fails")[0].message.clone();
    assert!(
        message.starts_with("`Split` failed because `SlowBoom` failed"),
        "{message}"
    );
}

/// A failing sibling doesn't cancel the others of its wave, and the parent's own body and checks never run.
#[test]
fn a_failed_wave_lets_its_siblings_finish() {
    let (project, program) = installed("wave_siblings");
    let run = split(&project, &program, 0, 0, 8);
    let third = run.calls[2].run.as_ref().expect("the sibling ran");
    assert_eq!(third.outcome.as_ref().ok(), Some(&number_value(0)));
    assert_eq!(run.calls[3].status(), CallStatus::Skipped);
    assert_eq!(run.calls[3].callee, "Spin");
}

/// A goal reached through a parent reports every failed check of it, in source order (R-CHK-09, R-RUN-10).
#[test]
fn a_wrapped_failure_keeps_every_failed_check() {
    let (project, program) = installed("wrapped_checks");
    let failures = run(&project, &program, "Wrap", &[5], 2).result().expect_err("fails");
    assert_eq!(failures.len(), 2, "{failures:#?}");
    assert!(failures.iter().all(|d| d.code == Code::CheckFailed));
    assert!(
        failures
            .iter()
            .all(|d| d.message.starts_with("`Wrap` failed because `Checked` failed: "))
    );
    assert!(failures[0].message.contains("result > n"), "{}", failures[0].message);
    assert!(failures[1].message.contains("result < n"), "{}", failures[1].message);
}

/// A failed sibling's own failed calls are reported as notes like the first failure's, wherever it is in source order:
/// `Outer` calls `FastBoom` and `Split`, and both of `Split`'s calls fail too.
#[test]
fn a_failed_siblings_own_failures_are_notes_too() {
    let (project, program) = installed("sibling_subtree");
    let failures = run(&project, &program, "Outer", &[5], 3).result().expect_err("fails");
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0]
            .message
            .starts_with("`Outer` failed because `FastBoom` failed")
    );
    assert_eq!(
        failures[0].notes,
        [
            "`Outer` also failed because `Split` failed because `SlowBoom` failed: `SlowBoom` tried to divide 25 by 0, \
             which has no answer. [VL0602]",
            "`Split` also failed because `FastBoom` failed: `FastBoom` tried to divide 25 by 0, which has no answer. \
             [VL0602]",
        ]
    );
}
