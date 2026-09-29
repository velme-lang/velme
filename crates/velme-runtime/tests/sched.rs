//! The wave scheduler (`runtime/30` §4, §5) on hand-written IR: waves, concurrency, and D-9's source-order failures,
//! whatever `--jobs` is.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value as Json, json};
use velme_builtins::limits::{FUEL_PER_MS, MAX_WALL_CLOCK_MS};
use velme_builtins::{BUILTINS_VERSION, Number, Value};
use velme_diagnostics::Code;
use velme_ir::{IR_VERSION, calls};
use velme_runtime::{
    CallStatus, Clock, GoalRun, Lock, Options, Registry, Store, load, run_goal, run_goal_peak, test_leaf,
};
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

goal Heavy(n: Number) -> Number:
    plan: \"Return n.\"
    check:
        - sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) > 0
    examples:
        - Heavy(1) == 1

goal Repeat(n: Number) -> Number:
    plan: \"Return n.\"
    check:
        - sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) > 0
    examples:
        - Repeat(1) == 1
        - Repeat(2) == 2
        - Repeat(3) == 3
        - Repeat(4) == 4

goal Trio(n: Number) -> Number:
    budget cpu=1ms
    plan: \"Return n.\"
    check:
        - n == n
        - n >= 0
        - sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) > 0

goal Tight(n: Number) -> Number:
    budget cpu=1ms
    plan: \"Count up to n.\"

goal Capped(n: Number) -> Number:
    budget cpu=1ms
    call:
        c = Double(n)
    plan: \"Add up ranges.\"

goal Roomy(n: Number) -> Number:
    budget cpu=1ms
    call:
        c = Double(n)
    plan: \"Add up ranges.\"

goal Greedy(n: Number) -> Number:
    budget cpu=1ms
    plan: \"Return n.\"
    check:
        - sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) + sum(range(10000)) > 0

goal Sink(xs: List<Number>) -> Number:
    plan: \"Return zero.\"

goal Bulky(n: Number) -> Number:
    budget memory=1kb
    call:
        s = Sink([1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1])
    plan: \"Sink a list bigger than the budget.\"

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

/// `pad` negations of `c`, then five `sum(range(n))`, added: with `n` 9997, `21 + 10n + pad` fuel, so 100 000 for a `pad`
/// of 9, the whole of `cpu=1ms`.
fn edge(pad: usize) -> Json {
    let sum = json!({"kind": "builtin", "name": "sum",
                     "args": [{"kind": "builtin", "name": "range", "args": [input("n")]}]});
    let start = (0..pad).fold(local("c"), |arg, _| json!({"kind": "unary", "op": "neg", "arg": arg}));
    (0..5).fold(start, |left, _| binary("add", left, sum.clone()))
}

/// The hand-written IR of goal `name` of [`SOURCE`], with the compiler's calls.
fn ir(program: &Program, name: &str) -> String {
    let (inputs, body) = match name {
        "Spin" => (json!([["n", number()]]), count("n")),
        "SlowBoom" | "FastBoom" => (json!([["n", number()]]), binary("div", count("n"), literal(0))),
        "Heavy" | "Greedy" | "Repeat" | "Trio" => (json!([["n", number()]]), input("n")),
        "Tight" => (json!([["n", number()]]), count("n")),
        "Capped" => (json!([["n", number()]]), edge(9)),
        "Roomy" => (json!([["n", number()]]), edge(8)),
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
        "Sink" => (json!([["xs", json!({"t": "List", "of": number()})]]), literal(0)),
        "Bulky" => (json!([["n", number()]]), local("s")),
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
    run_with(project, program, goal, inputs, Options::default().with_jobs(jobs))
}

/// [`run`] with `options`.
fn run_with(project: &Path, program: &Program, goal: &str, inputs: &[i64], options: Options) -> GoalRun {
    let id = goal_id(program, goal);
    let lock = Lock::read(project).expect("lock").expect("a lock");
    let registry = Registry::load(program, id, FILE, &lock, &Store::new(project)).expect("registry");
    let inputs = inputs.iter().map(|n| Value::from(Number::from(*n))).collect();
    run_goal(program, id, SOURCE, &registry, inputs, options)
}

/// [`run`], and the most bodies evaluated at once.
fn run_peak(project: &Path, program: &Program, goal: &str, inputs: &[i64], jobs: usize) -> (GoalRun, usize) {
    let id = goal_id(program, goal);
    let lock = Lock::read(project).expect("lock").expect("a lock");
    let registry = Registry::load(program, id, FILE, &lock, &Store::new(project)).expect("registry");
    let inputs = inputs.iter().map(|n| Value::from(Number::from(*n))).collect();
    run_goal_peak(
        program,
        id,
        SOURCE,
        &registry,
        inputs,
        Options::default().with_jobs(jobs),
    )
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
    // The trace shows the three children's spans overlapping, listed in source order (AC-RDM-03, `runtime/30` §8).
    let trace = parallel.trace(FILE).to_json();
    let calls = trace["goal"]["calls"].as_array().expect("calls");
    let bindings: Vec<&str> = calls.iter().map(|c| c["binding"].as_str().expect("binding")).collect();
    assert_eq!(bindings, ["a", "b", "c"]);
    let spans: Vec<(u64, u64)> = calls
        .iter()
        .map(|c| {
            let run = &c["run"];
            (
                run["start_us"].as_u64().expect("start"),
                run["end_us"].as_u64().expect("end"),
            )
        })
        .collect();
    for (i, (start, end)) in spans.iter().enumerate() {
        for (other_start, other_end) in &spans[i + 1..] {
            assert!(start < other_end && other_start < end, "{spans:?}");
        }
    }
}

/// The trace as JSON without its timing fields, which differ from run to run (`runtime/30` §8): they end in `_us`.
fn without_durations(json: &Json) -> Json {
    match json {
        Json::Object(map) => Json::Object(
            map.iter()
                .filter(|(key, _)| !key.ends_with("_us"))
                .map(|(key, value)| (key.clone(), without_durations(value)))
                .collect(),
        ),
        Json::Array(items) => Json::Array(items.iter().map(without_durations).collect()),
        other => other.clone(),
    }
}

/// The trace, without durations, is byte-identical for one worker and for eight, including the two failing siblings
/// where the slower is first in source order, and it lists every call in source order with its real outcome
/// (AC-RUN-04 trace half, D-9).
#[test]
fn ac_run_04_the_trace_is_the_same_for_every_job_count() {
    let (project, program) = installed("ac_run_04_trace");
    let cases: [(&str, &[i64]); 5] = [
        ("Main", &[4]),
        ("Fan", &[100]),
        ("Split", &[250, 0]),
        ("Split", &[0, 250]),
        ("Split", &[100, 100]),
    ];
    for (goal, inputs) in cases {
        let one = without_durations(&run(&project, &program, goal, inputs, 1).trace(FILE).to_json()).to_string();
        for jobs in [2, 8] {
            let many =
                without_durations(&run(&project, &program, goal, inputs, jobs).trace(FILE).to_json()).to_string();
            assert_eq!(one, many, "{goal} {inputs:?} with {jobs} jobs");
        }
    }
    let trace = run(&project, &program, "Split", &[250, 0], 8).trace(FILE).to_json();
    let calls = trace["goal"]["calls"].as_array().expect("calls");
    let seen: Vec<(&str, &str)> = calls
        .iter()
        .map(|c| {
            (
                c["binding"].as_str().expect("binding"),
                c["outcome"].as_str().expect("outcome"),
            )
        })
        .collect();
    // Both siblings' real outcomes appear, and the wave that never started is `skipped`, not `cancelled`.
    assert_eq!(
        seen,
        [
            ("first", "failed"),
            ("second", "failed"),
            ("third", "ok"),
            ("after", "skipped")
        ]
    );
    let failure = &trace["goal"]["failure"];
    assert_eq!(failure["code"], "VL0602");
    assert_eq!(failure["path"], json!(["Split", "SlowBoom"]));
}

/// Same source, inputs and lock: the result and the trace without durations are byte-identical over 100 runs, with
/// the workers varying (AC-RDM-09 interpreter half, INV-3).
#[test]
fn ac_rdm_09_a_hundred_runs_give_the_same_result_and_trace() {
    let (project, program) = installed("ac_rdm_09");
    for (goal, inputs) in [("Main", &[4][..]), ("Fan", &[30]), ("Split", &[40, 0])] {
        let mut reference: Option<(String, String)> = None;
        for i in 0..100 {
            let done = run(&project, &program, goal, inputs, [1, 3, 8][i % 3]);
            let seen = (
                format!("{:?}", done.result()),
                without_durations(&done.trace(FILE).to_json()).to_string(),
            );
            assert_eq!(reference.get_or_insert_with(|| seen.clone()), &seen, "{goal} run {i}");
        }
    }
}

/// A trace holds each goal's fuel and memory, its inputs, the arguments of each call and each check's text and verdict
/// (`runtime/30` §8), and a failed check's values.
#[test]
fn the_trace_holds_the_fields_of_runtime_30_section_8() {
    let (project, program) = installed("trace_fields");
    let main = run(&project, &program, "Main", &[4], 1).trace(FILE).to_json();
    assert_eq!(main["version"], 1);
    assert_eq!(main["file"], FILE);
    assert_eq!(main["reproducible"], true);
    let goal = &main["goal"];
    assert_eq!(goal["goal"], "Main");
    assert_eq!(goal["kind"], "wired");
    assert!(goal["artifact"].as_str().expect("a hash").starts_with("b3:"));
    assert_eq!(goal["inputs"], json!([{"name": "x", "value": 4}]));
    assert_eq!((&goal["outcome"], &goal["value"]), (&json!("ok"), &json!(9)));
    let second = &goal["calls"][1];
    assert_eq!(
        (&second["binding"], &second["goal"], &second["wave"]),
        (&json!("result"), &json!("AddOne"), &json!(2))
    );
    assert_eq!((&second["args"], &second["value"]), (&json!([{"value": 8}]), &json!(9)));
    assert_eq!(second["run"]["kind"], "leaf");
    assert!(second["run"]["fuel"].as_u64().expect("fuel") > 0);
    // `Checked`'s two checks contradict each other: the first fails and its values are in the trace.
    let wrap = run(&project, &program, "Wrap", &[3], 1).trace(FILE).to_json();
    let checked = &wrap["goal"]["calls"][0]["run"];
    assert_eq!(checked["outcome"], "failed");
    assert_eq!(checked["failure"]["code"], "VL0501");
    let checks = checked["checks"].as_array().expect("checks");
    assert_eq!(checks.len(), 2);
    assert_eq!(
        (&checks[0]["text"], &checks[0]["passed"]),
        (&json!("result > n"), &json!(false))
    );
    assert!(
        checks[0]["values"]
            .as_array()
            .expect("values")
            .iter()
            .any(|v| v["text"] == "result" && v["value"] == 3)
    );
    assert_eq!(wrap["goal"]["failure"]["path"], json!(["Wrap", "Checked"]));
    assert_eq!(wrap["goal"]["outcome"], "failed");
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

// ---- budgets (M4b) ----

/// The code of the failure a run reports, if it failed.
fn code_of(run: &GoalRun) -> Option<Code> {
    run.outcome.as_ref().err().and_then(|failed| failed.code())
}

/// A clock that reads the milliseconds it is given in turn, and the last of them for every read after (`runtime/30`
/// R-RUN-24). Every read is at a fixed point of the run, so a run over one goal reads it the same way every time.
#[derive(Debug)]
struct Script {
    readings: Vec<u64>,
    next: AtomicUsize,
}

impl Script {
    fn new(readings: &[u64]) -> Arc<Script> {
        Arc::new(Script {
            readings: readings.to_vec(),
            next: AtomicUsize::new(0),
        })
    }
}

impl Clock for Script {
    fn now(&self) -> Duration {
        let i = self.next.fetch_add(1, Ordering::SeqCst);
        let ms = self.readings.get(i).or(self.readings.last()).copied().unwrap_or(0);
        Duration::from_millis(ms)
    }
}

/// A clock that advances `step` milliseconds each time it is read.
#[derive(Debug)]
struct Ticking {
    step: u64,
    reads: AtomicU64,
}

impl Clock for Ticking {
    fn now(&self) -> Duration {
        Duration::from_millis(self.reads.fetch_add(1, Ordering::SeqCst) * self.step)
    }
}

/// Every file under `dir` with its bytes.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), fs::read(&path).expect("file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

/// An over-budget leaf, a `reduce` over `range(10000)` whose lambda reduces over `range(10000)`, fails with `VL0601`
/// deterministically, for every `--jobs`, and is no timeout (AC-RUN-05, AC-RDM-07).
#[test]
fn ac_run_05_an_over_budget_leaf_fails_with_vl0601_every_time() {
    let (project, program) = installed("ac_run_05");
    let first = run(&project, &program, "Spin", &[10_000], 1);
    assert_eq!(code_of(&first), Some(Code::BudgetExceeded));
    assert_eq!(
        first.result().expect_err("out of fuel")[0].message,
        "`Spin` took too many steps and was stopped."
    );
    assert!(first.reproducible());
    for jobs in [1, 4] {
        assert_eq!(run(&project, &program, "Spin", &[10_000], jobs), first);
    }
    // The trace shows the same non-zero fuel figure, the whole limit, every time.
    let fuel = first.trace(FILE).to_json()["goal"]["fuel"].as_u64().expect("fuel");
    assert_eq!(fuel, 10_000_000);
    for jobs in [1, 4] {
        let again = run(&project, &program, "Spin", &[10_000], jobs);
        assert_eq!(again.fuel, fuel);
    }
}

/// A failed goal's trace shows the work it did before it failed, and checks are charged to the invocation's budget:
/// a check that runs out of fuel reports the limit, not the body's figure (R-CHK-08, `runtime/30` §8).
#[test]
fn a_failed_goals_trace_shows_the_work_it_did() {
    let (project, program) = installed("failed_fuel");
    let ok = run(&project, &program, "Spin", &[5], 1);
    let boom = run(&project, &program, "FastBoom", &[5], 1);
    assert_eq!(code_of(&boom), Some(Code::ArithmeticError));
    // `FastBoom` counts as `Spin` does, then divides: at least what the count cost.
    assert!(ok.fuel > 0 && boom.fuel >= ok.fuel, "{} {}", ok.fuel, boom.fuel);
    let greedy = run(&project, &program, "Greedy", &[1], 1);
    assert_eq!(code_of(&greedy), Some(Code::BudgetExceeded));
    // `budget cpu=1ms` is 100 000 fuel; the body used 1, the checks the rest.
    assert_eq!(greedy.fuel, FUEL_PER_MS);
    let heavy = run(&project, &program, "Heavy", &[1], 1);
    assert!(heavy.fuel > 60_000, "{}", heavy.fuel);
    // A passing goal's figure includes its checks too.
    let plain = run(&project, &program, "Spin", &[1], 1);
    assert!(heavy.fuel > plain.fuel);
}

/// `x / 0` is `VL0602` and no value comes out of the run, however far the other goals got (AC-RUN-08).
#[test]
fn ac_run_08_division_by_zero_gives_no_value() {
    let (project, program) = installed("ac_run_08");
    let run = run(&project, &program, "FastBoom", &[5], 1);
    assert_eq!(code_of(&run), Some(Code::ArithmeticError));
    assert!(run.outcome.is_err());
    let diagnostics = run.result().expect_err("no value");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].message,
        "`FastBoom` tried to divide 25 by 0, which has no answer."
    );
    // A caller of the failing goal has no value either, though its other call succeeded.
    let outer = self::run(&project, &program, "Outer", &[5], 2);
    assert_eq!(code_of(&outer), Some(Code::ArithmeticError));
    assert!(outer.result().is_err());
    assert!(outer.reproducible());
}

/// `budget cpu=1ms` is 100 000 fuel: a goal that needs more fails with `VL0601`, and the same goal without the line
/// succeeds (AC-RUN-09).
#[test]
fn ac_run_09_a_cpu_budget_of_one_millisecond_is_a_hundred_thousand_fuel() {
    let (project, program) = installed("ac_run_09");
    assert_eq!(FUEL_PER_MS, 100_000);
    assert_eq!(program.goals[goal_id(&program, "Tight").0].budget.max_fuel, 100_000);
    // 200 × 200 elements at four fuel each is more than 100 000.
    let tight = run(&project, &program, "Tight", &[200], 1);
    assert_eq!(code_of(&tight), Some(Code::BudgetExceeded));
    let spin = run(&project, &program, "Spin", &[200], 1);
    assert_eq!(spin.result(), Ok(number_value(40_000)));
    // Within it, the line changes nothing.
    assert_eq!(
        run(&project, &program, "Tight", &[20], 1).result(),
        Ok(number_value(400))
    );
}

/// The arguments of a goal's calls spend the goal's own fuel, before its body does (`runtime/30` R-RUN-17): `Roomy`'s
/// body takes 99 999 of its 100 000 and its one argument the last unit, so it just fits; `Capped`'s body takes all
/// 100 000, so with the argument it is one over, though each fits alone.
#[test]
fn a_calls_arguments_spend_the_callers_fuel() {
    let (project, program) = installed("call_arguments_fuel");
    let roomy = run(&project, &program, "Roomy", &[9997], 1);
    // `c` is 2 × 9997, negated eight times, and 5 × the sum of 0 to 9996 is added to it.
    assert_eq!(roomy.result(), Ok(number_value(2 * 9997 + 5 * (9996 * 9997 / 2))));
    assert_eq!(statuses(&roomy), ["c:ok"]);
    let capped = run(&project, &program, "Capped", &[9997], 1);
    assert_eq!(code_of(&capped), Some(Code::BudgetExceeded));
    // The call ran; it is the parent's own body that ran out.
    assert_eq!(statuses(&capped), ["c:ok"]);
    let failed = capped.outcome.as_ref().expect_err("out of fuel");
    assert_eq!(
        failed.diagnostics()[0].message,
        "`Capped` took too many steps and was stopped."
    );
}

/// A call's arguments are the parent's expressions: one that allocates past the parent's memory limit fails the
/// parent with `VL0604`, and its trace shows the memory it spent, clamped to the limit (`runtime/30` §7.1, §8).
#[test]
fn a_memory_failure_in_an_argument_shows_the_memory_spent() {
    let (project, program) = installed("call_arguments_memory");
    let bulky = run(&project, &program, "Bulky", &[1], 1);
    assert_eq!(code_of(&bulky), Some(Code::MemoryLimitExceeded));
    assert_eq!(bulky.memory, 1024);
    assert!(bulky.fuel > 0);
}

/// A wall-clock timeout is `VL0603`, `reproducible: false`, and writes no artifact and no cache (AC-RUN-11, AC-RDM-07).
#[test]
fn ac_run_11_a_wall_clock_timeout_is_not_reproducible_and_writes_nothing() {
    let (project, program) = installed("ac_run_11");
    let before = tree(&project);
    // Read once at the start, once before the body, then at each look of the fuel meter: 60 001 ms is the second.
    let options = Options {
        clock: Script::new(&[0, 30_000, 60_000, 60_001]),
        ..Options::default().with_jobs(1)
    };
    let run = run_with(&project, &program, "Spin", &[300], options);
    assert_eq!(code_of(&run), Some(Code::Timeout));
    assert_eq!(
        run.result().expect_err("timed out")[0].message,
        "`Spin` ran too long and was stopped."
    );
    assert!(!run.reproducible());
    assert_eq!(run.trace(FILE).to_json()["reproducible"], false);
    // Whatever would store the run — a cache entry, a verdict, a saved trace — is only ever called for a reproducible
    // one: a timeout is handed to nothing, and the same path does take a run that finished.
    let mut stored = Vec::new();
    assert_eq!(run.persist_if_reproducible(|r| stored.push(r.goal.clone())), None);
    assert!(stored.is_empty());
    let finished = run_with(&project, &program, "Spin", &[3], Options::default().with_jobs(1));
    assert_eq!(finished.trace(FILE).to_json()["reproducible"], true);
    assert!(
        finished
            .persist_if_reproducible(|r| stored.push(r.goal.clone()))
            .is_some()
    );
    assert_eq!(stored, ["Spin"]);
    assert_eq!(tree(&project), before, "a timeout leaves the project as it was");
    // Nothing is left of a goal's siblings either: past the limit before the first wave, `Fan` stops with `VL0603`.
    let options = Options {
        clock: Script::new(&[0, 61_000]),
        ..Options::default().with_jobs(2)
    };
    let fan = run_with(&project, &program, "Fan", &[300], options);
    assert_eq!(code_of(&fan), Some(Code::Timeout));
    assert!(fan.calls.iter().all(|call| call.run.is_none()));
    assert!(!fan.reproducible());
    // A timeout in a call is the run's too, whichever failure is reported: a child that ran out of time.
    let options = Options {
        clock: Script::new(&[0, 0, 61_000]),
        ..Options::default().with_jobs(1)
    };
    let fan = run_with(&project, &program, "Fan", &[300], options);
    assert_eq!(code_of(&fan), Some(Code::Timeout));
    assert!(fan.calls.iter().any(|call| call.run.is_some()));
    assert!(!fan.reproducible());
    assert_eq!(tree(&project), before);
}

/// With an injected clock, the run is stopped only once the clock passes 60 s, and a full `max_fuel` stays far below
/// that at `FUEL_PER_MS` (AC-RUN-12, D-51).
#[test]
fn ac_run_12_the_watchdog_stops_a_run_only_once_the_clock_passes_sixty_seconds() {
    let (project, program) = installed("ac_run_12");
    let limit = 60_000;
    assert_eq!(MAX_WALL_CLOCK_MS, limit);
    let elapsed = |readings: &[u64]| {
        let options = Options {
            clock: Script::new(readings),
            ..Options::default().with_jobs(1)
        };
        run_with(&project, &program, "Spin", &[300], options)
    };
    // Exactly 60 s is not past it, however many times it is looked at; the next millisecond is.
    let at_limit = elapsed(&[0, 30_000, limit]);
    assert_eq!(at_limit.result(), Ok(number_value(90_000)));
    assert!(at_limit.reproducible());
    let past = elapsed(&[0, 30_000, limit, limit + 1]);
    assert_eq!(code_of(&past), Some(Code::Timeout));
    // The clock's origin doesn't matter, only its readings' differences.
    let later = elapsed(&[1_000_000, 1_030_000, 1_000_000 + limit]);
    assert_eq!(later.result(), Ok(number_value(90_000)));
}

/// A wired goal of 128 invocations, each spending about a full `max_fuel` on a clock that advances a millisecond per
/// `FUEL_PER_MS`, completes without `VL0603` (AC-RUN-12, D-51). At the real `max_fuel` that is 128 × 10M fuel, minutes in
/// a debug build, so each leaf spends 201 204 fuel, a 49th of it, and the clock advances 49 ms per `FUEL_PER_MS`
/// (a read every `FUEL_PER_MS` of fuel): the same 12.8 s of the run against the 60 s limit.
#[test]
fn ac_run_12_a_hundred_and_twenty_eight_full_invocations_finish_within_the_watchdog() {
    const CALLS: usize = 127;
    const SCALE: u64 = 49;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("sched")
        .join("ac_run_12_wide");
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("project directory");
    let lines: String = (0..CALLS).map(|i| format!("        c{i} = Spin(n)\n")).collect();
    let source = format!(
        "language: velme/0.1\n\ngoal Spin(n: Number) -> Number:\n    plan: \"Count up to n.\"\n\n\
         goal Wide(n: Number) -> Number:\n    call:\n{lines}    plan: \"Take the first.\"\n"
    );
    let program = program(&source);
    for (name, body) in [("Spin", count("n")), ("Wide", local("c0"))] {
        let calls = calls(&program, goal_id(&program, name)).expect("calls");
        let ir = json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": name, "types": {},
                        "inputs": [["n", number()]], "output": number(), "calls": calls, "body": body});
        install(&dir, FILE, &program, &ir.to_string());
    }
    let id = goal_id(&program, "Wide");
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let registry = Registry::load(&program, id, FILE, &lock, &Store::new(&dir)).expect("registry");
    let clock = Arc::new(Ticking {
        step: SCALE,
        reads: AtomicU64::new(0),
    });
    let options = Options {
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..Options::default().with_jobs(4)
    };
    let run = run_goal(&program, id, &source, &registry, vec![number_value(200)], options);
    assert_eq!(run.calls.len(), CALLS);
    assert_eq!(run.result(), Ok(number_value(40_000)));
    assert!(run.reproducible());
    // Each leaf reads the clock about three times (its own check, two looks of the meter): its full share of fuel.
    let elapsed = clock.reads.load(Ordering::SeqCst) * SCALE;
    assert!(elapsed > 10_000 && elapsed < MAX_WALL_CLOCK_MS, "{elapsed} ms");
}

/// A check that takes long enough is stopped by the watchdog like the body, in a run and under `velme test`: checks
/// and examples spend the invocation's budget and are watched with it (`runtime/30` D-10, `language/13` R-CHK-08).
#[test]
fn a_check_is_stopped_by_the_watchdog_too() {
    let (project, program) = installed("watched_checks");
    // The body is a few fuel; the check needs 120 000, so it is looked at once, at 100 000.
    let ok = run(&project, &program, "Heavy", &[1], 1);
    assert_eq!(ok.result(), Ok(number_value(1)));
    let options = Options {
        clock: Script::new(&[0, 0, 61_000]),
        ..Options::default().with_jobs(1)
    };
    let run = run_with(&project, &program, "Heavy", &[1], options);
    assert_eq!(code_of(&run), Some(Code::Timeout));
    assert!(!run.reproducible());
    // `velme test`'s examples and their checks.
    let id = goal_id(&program, "Heavy");
    let lock = Lock::read(&project).expect("lock").expect("a lock");
    let locked = load(&program, id, FILE, &lock, &Store::new(&project)).expect("locked");
    assert_eq!(test_leaf(&program, id, SOURCE, &locked, &Options::default()), Ok(1));
    let options = Options {
        clock: Script::new(&[0, 61_000]),
        ..Options::default()
    };
    let failures = test_leaf(&program, id, SOURCE, &locked, &options).expect_err("timed out");
    assert!(failures.iter().any(|d| d.code == Code::Timeout), "{failures:?}");
}

/// The watchdog bounds one run, and each example of `velme test` is its own run (`runtime/30` D-51): examples that
/// together take more than 60 s of a fake clock, each well under it, all pass.
#[test]
fn each_example_is_its_own_run_for_the_watchdog() {
    let (project, program) = installed("example_watchdog");
    let id = goal_id(&program, "Repeat");
    let lock = Lock::read(&project).expect("lock").expect("a lock");
    let locked = load(&program, id, FILE, &lock, &Store::new(&project)).expect("locked");
    // The check of each example is looked at once, at 100 000 fuel; the clock moves 20 s per reading.
    let clock = Arc::new(Ticking {
        step: 20_000,
        reads: AtomicU64::new(0),
    });
    let options = Options {
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..Options::default()
    };
    assert_eq!(test_leaf(&program, id, SOURCE, &locked, &options), Ok(4));
    assert!(clock.reads.load(Ordering::SeqCst) * 20_000 > MAX_WALL_CLOCK_MS);
}

/// Checks that finished before one ran out of fuel stay in the trace, with the fuel they cost (R-CHK-08, `runtime/30`
/// §8).
#[test]
fn checks_finished_before_a_limit_stay_in_the_trace() {
    let (project, program) = installed("checks_before_limit");
    let trio = run(&project, &program, "Trio", &[1], 1);
    assert_eq!(code_of(&trio), Some(Code::BudgetExceeded));
    let checks = &trio.checks;
    assert_eq!(
        checks.iter().map(|c| (c.text.as_str(), c.passed)).collect::<Vec<_>>(),
        [("n == n", true), ("n >= 0", true)]
    );
    assert_eq!(trio.fuel, FUEL_PER_MS);
}
