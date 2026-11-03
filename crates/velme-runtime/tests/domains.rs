//! A program with domains builds end to end (`compiler/21` R-IR-24, `compiler/22` R-SYNTH-10, R-SYNTH-60): base-valued
//! replies in every reply format complete into the same IR, with the compiler's `domains` table and Domain references
//! in the signature, which both back ends run with domains erased to their bases.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value as Json, json};
use velme_builtins::Value;
use velme_ir::{Origin, Request, to_canonical_string, validate};
use velme_runtime::{Backend, BuildInput, Clock, Lock, Mode, Options, Status, Store, Wasm, build, eval_leaf};
use velme_synth::{ReplyFormat, Scripted, SynthOptions};
use velme_test_support::{PanicProvider, goal_id, install, program};

const FILE: &str = "loans.velme";

const SOURCE: &str = "language: velme/0.2

type Risk = Text one of \"LOW\", \"HIGH\"
type Age = Number where value >= 18 and value <= 120 and value is whole
type Applicant:
    age: Age
    risk: Risk

goal Band(applicant: Applicant) -> Risk:
    plan: \"HIGH under 30, else LOW.\"
    examples:
        - Band(Applicant(age: 25, risk: \"LOW\")) == \"HIGH\"
        - Band(Applicant(age: 40, risk: \"LOW\")) == \"LOW\"

goal Adults(xs: List<Number>) -> List<Age>:
    plan: \"Keep the whole numbers from 18 to 120.\"
    examples:
        - Adults([10, 20]) == [20]
";

fn band() -> Json {
    let age = json!({"kind": "field", "of": {"kind": "input", "name": "applicant"}, "field": "age"});
    json!({"kind": "if",
           "cond": {"kind": "binary", "op": "lt", "left": age, "right": {"kind": "literal", "type": {"t": "Number"}, "value": 30}},
           "then": {"kind": "literal", "type": {"t": "Text"}, "value": "HIGH"},
           "else": {"kind": "literal", "type": {"t": "Text"}, "value": "LOW"}})
}

fn adults() -> Json {
    let x = json!({"kind": "local", "name": "x"});
    let number = |n: i64| json!({"kind": "literal", "type": {"t": "Number"}, "value": n});
    let both = |left: Json, right: Json| json!({"kind": "binary", "op": "and", "left": left, "right": right});
    let compare = |op: &str, left: Json, right: Json| json!({"kind": "binary", "op": op, "left": left, "right": right});
    let whole = compare(
        "eq",
        json!({"kind": "builtin", "name": "floor", "args": [x.clone()]}),
        x.clone(),
    );
    let body = both(
        both(
            compare("ge", x.clone(), number(18)),
            compare("le", x.clone(), number(120)),
        ),
        whole,
    );
    json!({"kind": "filter", "list": {"kind": "input", "name": "xs"}, "fn": {"param": "x", "body": body}})
}

/// The replies, in goal order, as `format` spells them. `Band`'s carries a wrong `domains` table of the model's own
/// beside its body, which the compiler's replaces (AC-SYNTH-62, R3 of D3 fix round 1).
fn replies(format: ReplyFormat) -> Scripted {
    let wrong = json!({"Age": {"base": {"t": "Text"}}});
    let bodies = match format {
        ReplyFormat::IrJson => vec![json!({"body": band(), "domains": wrong}), json!({"body": adults()})],
        ReplyFormat::Notation => vec![
            json!({"body": "return \"HIGH\" if applicant.age < 30 else \"LOW\"", "domains": wrong}),
            json!({"body": "return [x for x in xs if x >= 18 and x <= 120 and floor(x) == x]"}),
        ],
    };
    Scripted::replies(bodies.into_iter().map(|b| b.to_string()))
}

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("domains").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

/// Builds the program in `format` and returns each goal's artifact IR, as canonical JSON.
fn built(format: ReplyFormat) -> (PathBuf, Vec<(String, Json)>) {
    let dir = project(format.name());
    let program = program(SOURCE);
    let backend = replies(format);
    let input = BuildInput {
        mode: Mode::Build,
        program: &program,
        source: SOURCE,
        project: &dir,
        file: FILE,
        backend: Some(&backend),
        options: SynthOptions {
            reply_format: format,
            max_retries: 0,
            ..SynthOptions::default()
        },
        run: Options::default(),
    };
    let report = build(&input, &mut || {});
    assert!(
        report.goals.iter().all(|g| matches!(g.status, Status::Built(_))),
        "{format:?}: {report:#?}"
    );
    let lock = Lock::read(&dir).expect("lock").expect("present");
    let irs = lock
        .entries()
        .iter()
        .map(|entry| {
            let artifact = Store::new(&dir).get(entry.artifact).expect("artifact");
            (entry.name.clone(), serde_json::to_value(artifact.ir).expect("IR"))
        })
        .collect();
    (dir, irs)
}

/// AC-IR-15, AC-IR-16, AC-SYNTH-62: every reply format builds the same IR, completed with the compiler's `domains`
/// table and Domain references in the signature, while the body stays base-valued.
#[test]
fn a_domain_program_builds_the_same_ir_in_every_reply_format() {
    let (_, json) = built(ReplyFormat::IrJson);
    let format = ReplyFormat::Notation;
    let (_, other) = built(format);
    let canonical = |irs: &[(String, Json)]| {
        irs.iter()
            .map(|(name, ir)| (name.clone(), to_canonical_string(ir).expect("canonical")))
            .collect::<Vec<_>>()
    };
    assert_eq!(canonical(&other), canonical(&json), "{format:?}");
    let band = &json.iter().find(|(name, _)| name == "Band").expect("Band").1;
    assert_eq!(
        band["domains"],
        json!({"Age": {"base": {"t": "Number"}, "lower": {"value": 18, "inclusive": true},
                       "upper": {"value": 120, "inclusive": true}, "whole": true},
               "Risk": {"base": {"t": "Text"}, "members": ["LOW", "HIGH"]}})
    );
    assert_eq!(band["output"], json!({"t": "Domain", "name": "Risk"}));
    assert_eq!(
        band["types"]["Applicant"]["fields"][0][1],
        json!({"t": "Domain", "name": "Age"})
    );
    let adults = &json.iter().find(|(name, _)| name == "Adults").expect("Adults").1;
    assert_eq!(
        adults["output"],
        json!({"t": "List", "of": {"t": "Domain", "name": "Age"}})
    );
    assert!(!to_canonical_string(&adults["body"]).expect("body").contains("Domain"));
}

/// The built goals run on the interpreter and on WASM alike, a domain value being its base (R-IR-24).
#[test]
fn a_domain_program_runs_on_both_back_ends() {
    let (_, irs) = built(ReplyFormat::IrJson);
    let program = program(SOURCE);
    let wasm = Arc::new(Wasm::new(None));
    let numbers = |ns: &[i64]| {
        Value::list(
            ns.iter()
                .map(|n| Value::from(velme_builtins::Number::from(*n)))
                .collect(),
        )
    };
    for backend in [Backend::Interp, Backend::Wasm(Arc::clone(&wasm))] {
        for (name, ir) in &irs {
            let id = goal_id(&program, name);
            let request = Request {
                program: &program,
                goal: id,
                calls: &[],
                origin: Origin::Complete,
            };
            let valid = validate(&ir.to_string(), &request).expect("valid IR");
            let (inputs, expected) = if name == "Adults" {
                (vec![numbers(&[10, 20, 30, 121])], numbers(&[20, 30]))
            } else {
                let applicant = Value::record(
                    "Applicant",
                    vec![
                        ("age".to_owned(), Value::from(velme_builtins::Number::from(25_i64))),
                        ("risk".to_owned(), Value::text("LOW")),
                    ],
                );
                (vec![applicant], Value::text("HIGH"))
            };
            let (value, _) = eval_leaf(
                &backend,
                &program.goals[id.0],
                &valid,
                inputs,
                velme_builtins::execution::Limits::SYSTEM,
                None,
            );
            assert_eq!(value.expect("a value"), expected, "{name}");
        }
    }
}

/// AC-SYNTH-71: a lock hit generates the inputs it verifies on too. A domain predicate out of budget there ends the goal
/// with its own code, at the domain, naming the goal, with no provider built: no verdict on the locked IR.
#[test]
fn ac_synth_71_a_budget_stop_while_verifying_a_lock_hit_reaches_no_provider() {
    let source = "language: velme/0.2

type Busy = Number where every i in range(10000) has every j in range(10000) has i + j >= 0

goal Keep(n: Busy) -> Number:
    plan: \"Return n.\"
";
    let dir = project("ac_synth_71");
    let program = program(source);
    let mut ir =
        serde_json::to_value(velme_ir::wired_goal(&program, goal_id(&program, "Keep")).expect("IR")).expect("IR");
    ir["body"] = json!({"kind": "input", "name": "n"});
    install(&dir, FILE, &program, &ir.to_string());
    let input = BuildInput {
        mode: Mode::Build,
        program: &program,
        source,
        project: &dir,
        file: FILE,
        backend: Some(&PanicProvider),
        options: SynthOptions::default(),
        run: Options::default(),
    };
    let report = build(&input, &mut || {});
    let keep = &report.goals[0];
    assert_eq!(keep.status, Status::Failed, "{report:#?}");
    let d = &keep.diagnostics[0];
    assert_eq!(d.code, velme_diagnostics::Code::BudgetExceeded, "{d:?}");
    assert_eq!(d.message, "Checking whether a value is a `Busy` took too many steps.");
    assert!(d.notes.iter().any(|n| n.contains("`Keep`")), "{d:?}");
    assert!(!dir.join(".velme/synth-log.jsonl").exists(), "no request, no log line");
}

/// Gate F5: `admit`'s walk is watched like the scheduler's. On a clock that is always past `max_wall_clock`, a domain
/// predicate that polls ends the goal with `VL0603` before any request, not with a budget code.
#[test]
fn gate_f5_a_walk_timeout_in_admit_is_vl0603() {
    #[derive(Debug)]
    struct Jumping(AtomicU64);
    impl Clock for Jumping {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.fetch_add(1000, Ordering::SeqCst))
        }
    }
    let source = "language: velme/0.2

type Busy = Number where every i in range(10000) has every j in range(2) has i + j >= 0

goal Keep(n: Busy) -> Number:
    plan: \"Return n.\"
";
    let dir = project("gate_f5");
    let program = program(source);
    let script = Scripted::replies(["{}".to_owned()]);
    let input = BuildInput {
        mode: Mode::Build,
        program: &program,
        source,
        project: &dir,
        file: FILE,
        backend: Some(&script),
        options: SynthOptions::default(),
        run: Options {
            clock: Arc::new(Jumping(AtomicU64::new(0))),
            ..Options::default()
        },
    };
    let report = build(&input, &mut || {});
    let keep = &report.goals[0];
    assert_eq!(keep.status, Status::Failed, "{report:#?}");
    assert_eq!(
        keep.diagnostics[0].code,
        velme_diagnostics::Code::Timeout,
        "{:?}",
        keep.diagnostics
    );
    assert!(!dir.join(".velme/synth-log.jsonl").exists(), "no request, no log line");
}
