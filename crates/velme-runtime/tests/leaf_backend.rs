//! Leaf goal bodies on the WASM backend through the scheduler (`runtime/31` R-SBX-02, R-SBX-15..18, D-117, D-118):
//! the same value, failure, checks, fuel, memory and trace as on the interpreter, with a composite's tail and every
//! check still the interpreter's.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value as Json, json};
use velme_builtins::execution::Limits;
use velme_builtins::{BUILTINS_VERSION, Number, Value};
use velme_diagnostics::Code;
use velme_ir::{IR_VERSION, calls};
use velme_runtime::{Backend, Clock, GoalRun, Lock, Options, Registry, Store, Wasm, eval_leaf, run_goal};
use velme_sema::hir::Program;
use velme_test_support::differential::{corpus, differential};
use velme_test_support::{goal_id, install, program, valid_ir};

const FILE: &str = "leaves.velme";

/// `Spin` counts `n * n`; `Tight` too, within `cpu=1ms`; `Boom` divides by zero; `Checked` fails both its checks;
/// `Items` and `Many` make `range(n)`, one within `memory=1kb`; `Echo` returns its text; `Hidden` finds in a list of
/// `Nothing`, which the emitter declines; `Main` is wired and `Fan` composite.
const SOURCE: &str = "language: velme/0.1

goal Spin(n: Number) -> Number:
    plan: \"Count up to n.\"

goal Tight(n: Number) -> Number:
    budget cpu=1ms
    plan: \"Count up to n.\"

goal Boom(n: Number) -> Number:
    plan: \"Divide n by zero.\"

goal Checked(n: Number) -> Number:
    plan: \"Return n.\"
    check:
        - result > n
        - result < n

goal Items(n: Number) -> List<Number>:
    budget memory=1kb
    plan: \"Count to n.\"

goal Many(n: Number) -> List<Number>:
    plan: \"Count to n.\"

goal Echo(t: Text) -> Text:
    plan: \"Return t.\"

goal Hidden(xs: List<Number>) -> Boolean:
    plan: \"Say whether a list of nothing has nothing in it.\"

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
";

fn number() -> Json {
    json!({"t": "Number"})
}

fn numbers() -> Json {
    json!({"t": "List", "of": number()})
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

fn range(n: &str) -> Json {
    json!({"kind": "builtin", "name": "range", "args": [input(n)]})
}

/// `n * n`, counted up by one per element of `range(n)` inside each element of `range(n)`.
fn count(n: &str) -> Json {
    let inner = json!({"kind": "reduce", "list": range(n), "init": local("outer"),
                       "fn": {"acc": "sum", "param": "j", "body": binary("add", local("sum"), literal(1))}});
    json!({"kind": "reduce", "list": range(n), "init": literal(0),
           "fn": {"acc": "outer", "param": "i", "body": inner}})
}

/// `find(map(xs, x -> nothing), y -> true)` is empty: a `Nothing?`, which has no slot on WASM.
fn hidden() -> Json {
    let nothing = json!({"kind": "literal", "type": {"t": "Nothing"}, "value": null});
    let mapped = json!({"kind": "map", "list": input("xs"), "fn": {"param": "x", "body": nothing}});
    let yes = json!({"kind": "literal", "type": {"t": "Boolean"}, "value": true});
    let found = json!({"kind": "find", "list": mapped, "fn": {"param": "y", "body": yes}});
    json!({"kind": "unary", "op": "is_empty", "arg": found})
}

/// The hand-written IR of goal `name` of [`SOURCE`], with the compiler's calls.
fn ir(program: &Program, name: &str) -> String {
    let n = json!([["n", number()]]);
    let x = json!([["x", number()]]);
    let (inputs, output, body) = match name {
        "Spin" | "Tight" => (n, number(), count("n")),
        "Boom" => (n, number(), binary("div", input("n"), literal(0))),
        "Checked" => (n, number(), input("n")),
        "Items" | "Many" => (n, numbers(), range("n")),
        "Echo" => (json!([["t", {"t": "Text"}]]), json!({"t": "Text"}), input("t")),
        "Hidden" => (json!([["xs", numbers()]]), json!({"t": "Boolean"}), hidden()),
        "Double" => (x, number(), binary("mul", input("x"), literal(2))),
        "AddOne" => (x, number(), binary("add", input("x"), literal(1))),
        "Main" => (x, number(), local("result")),
        "Fan" => (
            n,
            number(),
            binary("add", binary("add", local("a"), local("b")), local("c")),
        ),
        _ => panic!("no goal {name}"),
    };
    let calls = calls(program, goal_id(program, name)).expect("calls");
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": name, "types": {},
           "inputs": inputs, "output": output, "calls": calls, "body": body})
    .to_string()
}

/// A project with every goal of [`SOURCE`] installed, and its checked program.
fn installed(name: &str) -> (PathBuf, Program) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("leaf_backend").join(name);
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

/// Runs `goal` of the program on `inputs` with `options`.
fn run(project: &Path, program: &Program, goal: &str, inputs: Vec<Value>, options: Options) -> GoalRun {
    let id = goal_id(program, goal);
    let lock = Lock::read(project).expect("lock").expect("a lock");
    let registry = Registry::load(program, id, FILE, &lock, &Store::new(project)).expect("registry");
    run_goal(program, id, SOURCE, &registry, inputs, options)
}

fn num(n: i64) -> Value {
    Value::from(Number::from(n))
}

/// Options for `backend` with `jobs` workers.
fn on(backend: &Backend, jobs: usize) -> Options {
    Options {
        backend: backend.clone(),
        ..Options::default().with_jobs(jobs)
    }
}

/// A WASM backend with no disk cache (R-SBX-20): a test never writes the user's.
fn wasm() -> Arc<Wasm> {
    Arc::new(Wasm::new(None))
}

fn code_of(run: &GoalRun) -> Option<Code> {
    run.outcome.as_ref().err().and_then(|failed| failed.code())
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

/// The run's result and its trace without durations, as text.
fn seen(run: &GoalRun) -> (String, String) {
    (
        format!("{:?}", run.result()),
        without_durations(&run.trace(FILE).to_json()).to_string(),
    )
}

/// Every golden leaf on its golden cases and every example leaf on its examples gives the identical value or full
/// diagnostic, fuel and memory on `interp` and `wasm`, also when one unit of fuel or one byte of memory short, and no
/// run nears its Wasmtime backstop (AC-SBX-01, R-SBX-15, D-118).
#[test]
fn ac_sbx_01_every_golden_and_example_leaf_is_the_same_on_both_backends() {
    let wasm = wasm();
    let (mut leaves, mut cases) = (0, 0);
    for leaf in corpus() {
        leaves += 1;
        for inputs in &leaf.cases {
            differential(&wasm, &leaf.program, leaf.goal, &leaf.ir, inputs)
                .unwrap_or_else(|e| panic!("{} on {inputs:?}:\n{e}", leaf.name));
            cases += 1;
        }
    }
    // The three golden leaves and the examples' fifteen.
    assert!(leaves >= 18 && cases >= 20, "{leaves} leaves, {cases} cases");
}

/// Same source, inputs and lock: on WASM the result and the trace without durations are byte-identical over 100 runs,
/// with the workers varying, and identical to the interpreter's, leaves and a composite's tail alike (AC-RDM-09 WASM
/// half, R-SBX-17, R-SBX-18).
#[test]
fn ac_rdm_09_a_hundred_runs_on_wasm_give_the_interpreters_result_and_trace() {
    let (project, program) = installed("ac_rdm_09");
    let wasm = Backend::Wasm(wasm());
    for (goal, inputs) in [("Main", vec![num(4)]), ("Fan", vec![num(30)]), ("Boom", vec![num(3)])] {
        let reference = seen(&run(&project, &program, goal, inputs.clone(), on(&Backend::Interp, 1)));
        for i in 0..100 {
            let done = run(&project, &program, goal, inputs.clone(), on(&wasm, [1, 3, 8][i % 3]));
            assert_eq!(seen(&done), reference, "{goal} run {i}");
        }
    }
}

/// The same failing check gives the identical report — values, order, fuel and memory — whichever backend ran the
/// body; the checks themselves are the interpreter's (AC-CHK-11, R-SBX-17).
#[test]
fn ac_chk_11_a_failing_check_reports_the_same_on_both_backends() {
    let (project, program) = installed("ac_chk_11");
    let interp = run(&project, &program, "Checked", vec![num(5)], on(&Backend::Interp, 1));
    assert_eq!(code_of(&interp), Some(Code::CheckFailed));
    assert_eq!(interp.checks.len(), 2);
    assert!(
        interp
            .checks
            .iter()
            .all(|check| !check.passed && !check.values.is_empty())
    );
    let wasm = wasm();
    for backend in [Backend::Wasm(Arc::clone(&wasm)), Backend::Auto(Arc::clone(&wasm))] {
        let other = run(&project, &program, "Checked", vec![num(5)], on(&backend, 1));
        assert_eq!(other, interp, "{backend:?}");
        assert_eq!(seen(&other), seen(&interp), "{backend:?}");
    }
}

/// Fuel, memory, list size and output size each stop a leaf on WASM with their own `VL06xx` code, at the same point
/// and with the same figures as on the interpreter; calls and depth are static (AC-SEC-07, D-117).
#[test]
fn ac_sec_07_each_limit_stops_a_leaf_on_wasm_with_its_own_code() {
    let (project, program) = installed("ac_sec_07");
    let wasm = Backend::Wasm(wasm());
    let long = Value::text(&"a".repeat(1_100_000));
    let cases = [
        ("Tight", vec![num(1000)], Code::BudgetExceeded),
        ("Items", vec![num(100)], Code::MemoryLimitExceeded),
        ("Many", vec![num(10_001)], Code::SizeLimitExceeded),
        ("Echo", vec![long], Code::SizeLimitExceeded),
    ];
    for (goal, inputs, code) in cases {
        let interp = run(&project, &program, goal, inputs.clone(), on(&Backend::Interp, 1));
        let other = run(&project, &program, goal, inputs, on(&wasm, 1));
        assert_eq!(code_of(&other), Some(code), "{goal}");
        assert_eq!(other, interp, "{goal}");
        assert_eq!(seen(&other), seen(&interp), "{goal}");
    }
}

/// A clock that reads the milliseconds it is given in turn, and the last of them for every read after.
#[derive(Debug)]
struct Script {
    readings: Vec<u64>,
    next: AtomicUsize,
}

impl Clock for Script {
    fn now(&self) -> Duration {
        let i = self.next.fetch_add(1, Ordering::SeqCst);
        let ms = self.readings.get(i).or(self.readings.last()).copied().unwrap_or(0);
        Duration::from_millis(ms)
    }
}

/// On WASM a deliberately expensive leaf is stopped by fuel with `VL0601` and the interpreter's figure, every time, and
/// by the injected clock past `max_wall_clock` with `VL0603`, which is not reproducible (AC-RDM-07 WASM half, D-115).
#[test]
fn ac_rdm_07_on_wasm_fuel_is_vl0601_and_the_watchdog_vl0603() {
    let (project, program) = installed("ac_rdm_07");
    let wasm = Backend::Wasm(wasm());
    let interp = run(&project, &program, "Spin", vec![num(10_000)], on(&Backend::Interp, 1));
    assert_eq!(code_of(&interp), Some(Code::BudgetExceeded));
    for _ in 0..3 {
        let stopped = run(&project, &program, "Spin", vec![num(10_000)], on(&wasm, 1));
        assert_eq!(stopped, interp);
        assert!(stopped.reproducible());
    }
    // Read at the start, before the body, then by the body's watchdog: past 60 s from there on.
    let options = Options {
        clock: Arc::new(Script {
            readings: vec![0, 0, 61_000],
            next: AtomicUsize::new(0),
        }),
        ..on(&wasm, 1)
    };
    let timed_out = run(&project, &program, "Spin", vec![num(10_000)], options);
    assert_eq!(code_of(&timed_out), Some(Code::Timeout));
    assert!(!timed_out.reproducible());
}

/// Under an explicit `wasm` a leaf the emitter declines is `VL0607`, saying why and how to run it; under `auto` it
/// runs on the interpreter, as if `interp` had been asked, with a note for `--verbose` (R-SBX-02, R-SBX-16, D-117).
#[test]
fn r_sbx_02_a_declined_leaf_is_vl0607_under_wasm_and_the_interpreters_under_auto() {
    let (project, program) = installed("r_sbx_02");
    let inputs = vec![Value::list(vec![num(1), num(2)])];
    let interp = run(&project, &program, "Hidden", inputs.clone(), on(&Backend::Interp, 1));
    assert_eq!(interp.result(), Ok(Value::Boolean(true)));
    let wasm = wasm();
    let declined = run(
        &project,
        &program,
        "Hidden",
        inputs.clone(),
        on(&Backend::Wasm(Arc::clone(&wasm)), 1),
    );
    let diagnostics = declined.result().expect_err("declined");
    assert_eq!(diagnostics[0].code, Code::InternalError);
    assert_eq!(
        diagnostics[0].notes,
        [
            "the WASM backend can't run `Hidden` yet: it has no code for a type that holds Nothing; run it without \
          `--backend wasm`"
        ]
    );
    assert!(wasm.notes().is_empty(), "{:?}", wasm.notes());
    let auto = run(
        &project,
        &program,
        "Hidden",
        inputs,
        on(&Backend::Auto(Arc::clone(&wasm)), 1),
    );
    assert_eq!(auto, interp);
    assert_eq!(
        wasm.notes(),
        ["`Hidden` ran on the interpreter: the WASM backend has no code for a type that holds Nothing"]
    );
}

/// A cache directory the sandbox refuses turns the disk cache off, and the run goes on the same; `--verbose` has a
/// note saying why (R-SBX-20, D-120).
#[test]
fn r_sbx_20_a_refused_cache_directory_is_a_note_and_changes_nothing() {
    let (project, program) = installed("r_sbx_20");
    let file = project.join("not-a-directory");
    fs::write(&file, "").expect("a file");
    let wasm = Arc::new(Wasm::new(Some(file)));
    let interp = run(&project, &program, "Main", vec![num(4)], on(&Backend::Interp, 1));
    let other = run(
        &project,
        &program,
        "Main",
        vec![num(4)],
        on(&Backend::Wasm(Arc::clone(&wasm)), 1),
    );
    assert_eq!(other, interp);
    let notes = wasm.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("is not used"), "{notes:?}");
}

/// Goals whose modules have the same bytes each keep their own types in one process: two records of one `Number`
/// field, and an echo of a `Text` and of a `List<Number>`, give on WASM what they give on the interpreter, in either
/// order (R-SBX-15, R-SBX-18).
#[test]
fn r_sbx_15_leaves_whose_modules_share_bytes_keep_their_own_types() {
    let source = "language: velme/0.1

type P:
    a: Number

type Q:
    b: Number

goal A(x: Number) -> P:
    plan: \"Make a P of x.\"

goal B(x: Number) -> Q:
    plan: \"Make a Q of x.\"

goal T(t: Text) -> Text:
    plan: \"Return t.\"

goal L(xs: List<Number>) -> List<Number>:
    plan: \"Return xs.\"
";
    let checked = program(source);
    let record = |goal: &str, ty: &str, field: &str| {
        json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal,
               "types": {ty: {"fields": [[field, number()]]}}, "inputs": [["x", number()]],
               "output": {"t": "Record", "name": ty}, "calls": [],
               "body": {"kind": "record", "type": ty, "fields": {field: input("x")}}})
    };
    let echo = |goal: &str, name: &str, ty: Json| {
        json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal, "types": {},
               "inputs": [[name, ty]], "output": ty, "calls": [], "body": input(name)})
    };
    let pairs = [
        (
            ("A", record("A", "P", "a"), num(7)),
            ("B", record("B", "Q", "b"), num(7)),
        ),
        (
            ("T", echo("T", "t", json!({"t": "Text"})), Value::text("ab")),
            ("L", echo("L", "xs", numbers()), Value::list(vec![num(1), num(2)])),
        ),
    ];
    for (first, second) in pairs {
        for order in [[&first, &second], [&second, &first]] {
            // A process of its own for each order: the first goal's module is the one compiled.
            let wasm = Backend::Wasm(wasm());
            let emitted: Vec<_> = order
                .iter()
                .map(|(_, ir, _)| valid_ir(&checked, &ir.to_string()))
                .collect();
            let bytes: Vec<_> = emitted
                .iter()
                .map(|ir| velme_wasm::emit(ir).expect("a module").bytes().to_vec())
                .collect();
            assert_eq!(bytes[0], bytes[1], "{} and {}", order[0].0, order[1].0);
            for ((goal, _, input), ir) in order.iter().zip(&emitted) {
                let target = &checked.goals[goal_id(&checked, goal).0];
                let on = |backend: &Backend| eval_leaf(backend, target, ir, vec![input.clone()], Limits::SYSTEM, None);
                assert_eq!(on(&wasm), on(&Backend::Interp), "{goal}");
            }
        }
    }
}
