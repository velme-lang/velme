//! The sandbox on emitted modules and on modules no emitter makes (`runtime/31` §5-§7, §10): what a leaf gives on
//! WASM, the whitelist, the limits and their backstops, the order a trap is read in, and the cache. The comparison
//! with the interpreter over the whole corpus is the differential suite's (R-SBX-15).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use velme_builtins::execution::{Error, Failure, Interrupt, Limits};
use velme_builtins::limits::{MAX_FUEL, MAX_LIST_SIZE, MAX_MEMORY};
use velme_builtins::{BUILTINS_VERSION, Number, TEXT_BLOCK_BYTES, Value};
use velme_diagnostics::{Code, Span};
use velme_ir::limits::{MAX_COLLECTION_NESTING, MAX_DEPTH};
use velme_ir::{Goal, IR_VERSION, calls, decode_str, encode_value, from_json_str};
use velme_test_support::{example_cases, goal_id, interpret, program, read, repo, valid_ir};
use wasm_encoder::BlockType::Empty;
use wasm_encoder::{EntityType, ImportSection, Instruction as I, TypeSection};

use crate::abi::{self, Import, RESULT, Reason};
use crate::cache::Refused;
#[cfg(unix)]
use crate::cache::Unusable;
use crate::code::{Assembly, Func, G_FUEL, G_REASON, V, mem};
use crate::runtime::{self, Rt};
use crate::sandbox::{BYTE_INSTRUCTIONS, MAX_LINKED, MEMORY_MOVES, SUM_ITEM_INSTRUCTIONS, memory_limit};
use crate::tests::{
    EVERYTHING, EXAMPLES, binary, builtin, collection, document, documents, everything, field, input, item, local,
    number, unary,
};
use crate::{
    Backstop, FUEL_ALLOWANCE, FUEL_FACTOR, LoadError, MEMORY_FACTOR, Module, Program, Run, Sandbox, UNIT_INSTRUCTIONS,
    backstop_fuel, emit,
};

/// A watchdog that never stops the run.
fn unwatched() -> Interrupt {
    Interrupt::new(|| false)
}

fn sandbox() -> Sandbox {
    Sandbox::new(None).expect("a sandbox")
}

/// A goal `G` of the given signature, written as `x: Number, y: Number -> Number`, with `body`; `types` are the
/// IR's record types.
fn goal(signature: &str, types: &str, body: &str) -> Module {
    let (params, output) = signature.split_once("->").expect("a signature");
    let declared = "type Item:\n    k: Number\n    name: Text\n\ntype Tag:\n    name: Text?\n    items: List<Item>\n";
    let source = format!(
        "language: velme/0.1\n\n{declared}\ngoal G({}) -> {}:\n    plan: \"x\"\n",
        params.trim(),
        output.trim()
    );
    let inputs: Vec<String> = params
        .split(", ")
        .map(|param| {
            let (name, ty) = param.split_once(':').expect("a parameter");
            format!(r#"["{}", {}]"#, name.trim(), ty_ir(ty))
        })
        .collect();
    let ir = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "G", "types": {{{types}}},
            "inputs": [{}], "output": {}, "body": {body}}}"#,
        inputs.join(", "),
        ty_ir(output)
    );
    emit(&valid_ir(&program(&source), &ir)).expect("a module")
}

/// A type as the IR writes it.
fn ty_ir(ty: &str) -> String {
    let ty = ty.trim();
    if let Some(inner) = ty.strip_suffix('?') {
        return format!(r#"{{"t": "Optional", "of": {}}}"#, ty_ir(inner));
    }
    if let Some(inner) = ty.strip_prefix("List<").and_then(|rest| rest.strip_suffix('>')) {
        return format!(r#"{{"t": "List", "of": {}}}"#, ty_ir(inner));
    }
    match ty {
        "Number" | "Text" | "Boolean" | "Nothing" => format!(r#"{{"t": "{ty}"}}"#),
        name => format!(r#"{{"t": "Record", "name": "{name}"}}"#),
    }
}

const ITEM: &str = r#""Item": {"fields": [["k", {"t": "Number"}], ["name", {"t": "Text"}]]}"#;
const TAG: &str = r#""Tag": {"fields": [["name", {"t": "Optional", "of": {"t": "Text"}}],
    ["items", {"t": "List", "of": {"t": "Record", "name": "Item"}}]]}"#;

fn num(n: i64) -> Value {
    Value::Number(n.into())
}

fn run(module: &Module, inputs: &[Value], limits: Limits) -> Run {
    sandbox().load(module).expect("loads").run(inputs, limits, &unwatched())
}

/// A module with the whole interface of an emitted one (R-SBX-03) whose `velme_run` is `body`, which leaves an
/// `i32`: what the emitter never makes.
fn hostile(body: impl FnOnce(&mut Func)) -> Vec<u8> {
    let mut assembly = Assembly::default();
    runtime::define(&mut assembly);
    let mut f = Func::new(&[V::I32], &[V::I32]);
    body(&mut f);
    let run = assembly.push(f);
    assembly.finish(&[], Rt::Alloc.index(), run).expect("a module")
}

/// Runs the hostile module `bytes` as if it were the goal `like`, on `inputs`.
fn run_hostile(bytes: &[u8], like: &Module, inputs: &[Value], limits: Limits) -> Run {
    let program = sandbox().load_bytes(bytes, like).expect("loads");
    program.run(inputs, limits, &unwatched())
}

fn internal() -> Result<Value, Failure> {
    Err(velme_builtins::Error::Internal.into())
}

/// A directory of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("velme-wasm-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The cached files of `dir`.
fn cached_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries.map(|entry| entry.expect("an entry").path()).collect();
    files.sort();
    files
}

// ---- the corpus ----

/// Each golden leaf on the cases of `tests/golden/ir/run/`, against what the interpreter's own snapshot of those
/// cases holds, fuel and memory included; and each leaf of the examples on its `examples`. No run comes near its
/// backstop.
#[test]
fn r_sbx_15_golden_and_example_leaves_give_their_outputs_on_wasm() {
    let sandbox = sandbox();
    let limits = Limits::SYSTEM;
    let quarter = |name: &str, run: &Run| {
        assert_eq!(run.backstop, None, "{name}");
        assert!(run.wasmtime_fuel.within_a_quarter(), "{name}: {run:?}");
    };
    let source = read(&repo("tests/golden/ir/goals.velme"));
    let golden = program(&source);
    let mut leaves = 0;
    for path in documents(&repo("tests/golden/ir/accept")) {
        let text = read(&path);
        let name = path.file_name().and_then(|n| n.to_str()).expect("a file name");
        let parsed: Goal = from_json_str(&text).expect("an IR document");
        let id = goal_id(&golden, &parsed.goal);
        if !calls(&golden, id).expect("a checked goal").is_empty() {
            continue;
        }
        leaves += 1;
        let loaded = sandbox
            .load(&emit(&valid_ir(&golden, &text)).expect("emits"))
            .expect("loads");
        let cases: serde_json::Value =
            from_json_str(&read(&repo(&format!("tests/golden/ir/run/{name}")))).expect("cases");
        let mut out = String::new();
        for case in cases.as_array().expect("a list of cases") {
            let params = golden.goals[id.0].params.iter();
            let inputs: Vec<Value> = params
                .map(|p| decode_str(&case["inputs"][&p.name].to_string(), &p.ty, &golden).expect("decodes"))
                .collect();
            let run = loaded.run(&inputs, limits, &unwatched());
            quarter(name, &run);
            let value = encode_value(run.result.as_ref().expect("runs")).expect("encodes");
            let (fuel, memory) = (run.spent.fuel, run.spent.memory);
            out.push_str(&format!(
                "{} -> {value} (fuel {fuel}, memory {memory})\n",
                case["inputs"]
            ));
        }
        let snapshot = read(&repo(&format!(
            "crates/velme-interp/tests/snapshots/interp__golden_ir_runs@{name}.snap"
        )));
        let expected = snapshot.splitn(3, "---\n").nth(2).expect("a snapshot body");
        assert_eq!(out.trim_end(), expected.trim_end(), "{name}");
    }
    assert!(leaves >= 3, "{leaves} golden leaves");

    let mut examples = 0;
    for (source, ir) in EXAMPLES {
        let text = read(&repo(source));
        let checked = program(&text);
        for path in documents(&repo(ir)) {
            let document = read(&path);
            let parsed: Goal = from_json_str(&document).expect("an IR document");
            let id = goal_id(&checked, &parsed.goal);
            if !calls(&checked, id).expect("a checked goal").is_empty() {
                continue;
            }
            let loaded = sandbox
                .load(&emit(&valid_ir(&checked, &document)).expect("emits"))
                .expect("loads");
            for (inputs, expected) in example_cases(&checked, &text, id) {
                let run = loaded.run(&inputs, limits, &unwatched());
                quarter(source, &run);
                assert_eq!(run.result, Ok(expected), "{source} {}", parsed.goal);
                examples += 1;
            }
        }
    }
    assert!(examples >= 10, "{examples} examples");
}

#[test]
fn every_node_kind_and_builtin_runs_within_a_quarter_of_the_backstop() {
    let ir = valid_ir(&program(EVERYTHING), &everything());
    let items = |items: &[(i64, &str)]| {
        let items = items.iter().map(|(k, name)| {
            Value::record(
                "Item",
                vec![("k".to_owned(), num(*k)), ("name".to_owned(), Value::text(name))],
            )
        });
        Value::list(items.collect())
    };
    let numbers = |xs: &[i64]| Value::list(xs.iter().copied().map(num).collect());
    let loaded = sandbox().load(&emit(&ir).expect("emits")).expect("loads");
    for inputs in [
        [
            numbers(&[1, 2, 3, -4]),
            items(&[(3, "c"), (1, "a"), (2, "b")]),
            Value::text("tail"),
            num(2),
        ],
        [numbers(&[]), items(&[]), Value::text(""), Value::Nothing],
        [
            numbers(&[3, 3, 0]),
            items(&[(3, "c")]),
            Value::text("héllo"),
            Value::Nothing,
        ],
    ] {
        let run = loaded.run(&inputs, Limits::SYSTEM, &unwatched());
        assert_eq!(run.backstop, None);
        assert!(run.wasmtime_fuel.within_a_quarter(), "{run:?}");
        // Whatever it gives, it is a value or a failure of the language, never the backend's.
        assert_ne!(run.result, internal(), "{inputs:?}");
        // The same run again, in a fresh store, is the same run (INV-3).
        assert_eq!(loaded.run(&inputs, Limits::SYSTEM, &unwatched()), run);
    }
}

#[test]
fn values_of_every_layout_cross_the_boundary_both_ways() {
    // R-SBX-03, §3: the host writes every input with its prefixes, optionals boxed, and reads the same back. With
    // no memory at all, too: an input is nobody's charge (D-53).
    let item = |k: i64, name: &str| {
        Value::record(
            "Item",
            vec![("k".to_owned(), num(k)), ("name".to_owned(), Value::text(name))],
        )
    };
    let tag = |name: Value, items: Vec<Value>| {
        let fields = vec![("name".to_owned(), name), ("items".to_owned(), Value::list(items))];
        Value::record_of("Tag", fields, &[true, false])
    };
    let types = format!("{ITEM}, {TAG}");
    let types = types.as_str();
    let big = Number::parse("-79228162514264337593543950335").expect("a number");
    let cases: Vec<(&str, &str, Vec<Value>)> = vec![
        ("x: Number -> Number", "", vec![num(7), Value::Number(big), num(0)]),
        (
            "x: Boolean -> Boolean",
            "",
            vec![Value::Boolean(true), Value::Boolean(false)],
        ),
        (
            "x: Text -> Text",
            "",
            vec![Value::text(""), Value::text("héllo wörld, nine bytes and more")],
        ),
        ("x: Number? -> Number?", "", vec![Value::Nothing, num(3)]),
        (
            "x: Text? -> Text?",
            "",
            vec![Value::Nothing, Value::text(""), Value::text("a")],
        ),
        ("x: Item -> Item", ITEM, vec![item(1, "a")]),
        ("x: Item? -> Item?", ITEM, vec![Value::Nothing, item(2, "bb")]),
        (
            "x: Tag -> Tag",
            types,
            vec![
                tag(Value::Nothing, vec![]),
                tag(Value::text("t"), vec![item(1, "a"), item(2, "")]),
            ],
        ),
        (
            "x: List<Number?> -> List<Number?>",
            "",
            vec![
                Value::list_of(vec![], true),
                Value::list_of(vec![num(1), Value::Nothing, num(2)], true),
            ],
        ),
        (
            "x: List<List<Text>> -> List<List<Text>>",
            "",
            vec![Value::list(vec![
                Value::list(vec![]),
                Value::list(vec![Value::text("a"), Value::text("")]),
            ])],
        ),
        (
            "x: List<Item?> -> List<Item?>",
            ITEM,
            vec![Value::list_of(vec![Value::Nothing, item(1, "a")], true)],
        ),
        (
            "x: List<Tag>? -> List<Tag>?",
            types,
            vec![
                Value::Nothing,
                Value::list(vec![tag(Value::text("t"), vec![item(1, "a")])]),
            ],
        ),
    ];
    let sandbox = sandbox();
    for (signature, types, values) in cases {
        let loaded = sandbox.load(&goal(signature, types, &input("x"))).expect("loads");
        for value in values {
            for memory in [0, MAX_MEMORY] {
                let run = loaded.run(std::slice::from_ref(&value), Limits { fuel: 10, memory }, &unwatched());
                assert_eq!(run.result, Ok(value.clone()), "{signature}");
                assert_eq!(
                    (run.spent.fuel, run.spent.memory, run.backstop),
                    (1, 0, None),
                    "{signature}"
                );
            }
        }
    }
}

#[test]
fn a_list_of_nothing_is_read_back_as_its_length() {
    let nothing = r#"{"kind": "literal", "type": {"t": "Nothing"}, "value": null}"#;
    let body = collection("map", &builtin("range", &[&input("x")]), "v", nothing);
    let source = "language: velme/0.1\n\ngoal G(x: Number) -> List<Nothing>:\n    plan: \"x\"\n";
    let ir = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "G", "types": {{}},
            "inputs": [["x", {{"t": "Number"}}]], "output": {{"t": "List", "of": {{"t": "Nothing"}}}},
            "body": {body}}}"#
    );
    let module = emit(&valid_ir(&program(source), &ir)).expect("a module");
    let run = run(&module, &[num(3)], Limits::SYSTEM);
    assert_eq!(run.result, Ok(Value::list(vec![Value::Nothing; 3])));
}

/// Two goals whose modules have the same bytes share what they were compiled to, never their signature: each program
/// reads its own output type (R-SBX-03).
#[test]
fn modules_with_the_same_bytes_keep_their_own_signatures() {
    let text = goal("t: Text -> Text", "", &input("t"));
    let list = goal("xs: List<Number> -> List<Number>", "", &input("xs"));
    assert_eq!(text.bytes(), list.bytes());
    let sandbox = sandbox();
    let text = sandbox.load(&text).expect("loads");
    let list = sandbox.load(&list).expect("loads");
    let numbers = Value::list(vec![num(1), num(2)]);
    let run = |program: &Program, input: &Value| {
        program
            .run(std::slice::from_ref(input), Limits::SYSTEM, &unwatched())
            .result
    };
    assert_eq!(run(&text, &Value::text("ab")), Ok(Value::text("ab")));
    assert_eq!(run(&list, &numbers), Ok(numbers));
}

/// A sandbox keeps at most `MAX_LINKED` compiled modules, the least recently used dropped first, and a dropped one is
/// compiled again and runs as before.
#[test]
fn the_modules_kept_in_memory_are_bounded() {
    // `-x` wrapped `k` times in `abs` or `neg`, by the bits of `k`: different code for each `k`, all of it small.
    let copies = |k: usize| {
        let mut body = unary("neg", &input("x"));
        for bit in 0..usize::BITS - k.leading_zeros() {
            body = if k >> bit & 1 == 1 {
                builtin("abs", &[&body])
            } else {
                unary("neg", &body)
            };
        }
        goal("x: Number -> Number", "", &body)
    };
    let sandbox = sandbox();
    let first = copies(1);
    sandbox.load(&first).expect("loads");
    // The rest from eight threads, to compile them sooner: `first` stays the least recently used either way.
    std::thread::scope(|scope| {
        for start in 0..8 {
            let (sandbox, copies) = (&sandbox, &copies);
            scope.spawn(move || {
                for k in (2 + start..=MAX_LINKED + 1).step_by(8) {
                    sandbox.load(&copies(k)).expect("loads");
                }
            });
        }
    });
    assert_eq!(sandbox.linked(), MAX_LINKED);
    assert!(!sandbox.is_kept(first.bytes()), "the least recently used is dropped");
    let again = sandbox.load(&first).expect("loads again");
    assert_eq!(sandbox.linked(), MAX_LINKED);
    assert!(sandbox.is_kept(first.bytes()));
    // One more: now some other module is the least recently used, and `first`, just loaded, stays.
    let last = copies(MAX_LINKED + 2);
    sandbox.load(&last).expect("loads");
    assert_eq!(sandbox.linked(), MAX_LINKED);
    assert!(sandbox.is_kept(first.bytes()) && sandbox.is_kept(last.bytes()));
    let run = again.run(&[num(7)], Limits::SYSTEM, &unwatched());
    assert_eq!(run.result, Ok(num(7)));
}

/// A run that failed in the host before `velme_run` says it never started, so the runtime may run the goal on the
/// interpreter; one that started says so whatever its outcome (D-121).
#[test]
fn a_run_says_whether_the_module_started() {
    let module = goal(
        "x: Number, y: Number -> Number",
        "",
        &binary("div", &input("x"), &input("y")),
    );
    assert!(run(&module, &[num(1), num(2)], Limits::SYSTEM).started);
    assert!(run(&module, &[num(1), num(0)], Limits::SYSTEM).started);
    // Inputs that are not the goal's can't be written into its memory.
    let unstarted = run(&module, &[Value::text("a"), num(0)], Limits::SYSTEM);
    assert!(!unstarted.started);
    assert_eq!(unstarted.result, internal());
}

// ---- the whitelist ----

/// A module that imports the whitelist and then `module.name`.
fn importing(module: &str, name: &str) -> Vec<u8> {
    let mut bytes = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    bytes.section(&types);
    let mut imports = ImportSection::new();
    imports.import(module, name, EntityType::Function(0));
    bytes.section(&imports);
    bytes.finish()
}

fn refused(sandbox: &Sandbox, module: &str, name: &str) {
    let like = goal("x: Number -> Number", "", &input("x"));
    let error = sandbox
        .load_bytes(&importing(module, name), &like)
        .expect_err("refused");
    let import = format!("{module}.{name}");
    assert_eq!(error, LoadError::Denied { import: import.clone() });
    assert_eq!(error.code(), Code::CapabilityDenied);
    let diagnostic = error.diagnostic("G", Span::new(0, 1));
    assert_eq!(diagnostic.code, Code::CapabilityDenied);
    assert!(
        diagnostic.message.contains(&format!("`{import}`")),
        "{}",
        diagnostic.message
    );
}

#[test]
fn ac_sbx_02_a_module_importing_wasi_is_refused_by_name_and_never_compiled() {
    let scratch = Scratch::new("whitelist");
    let cache = scratch.0.join("wasm");
    let sandbox = Sandbox::new(Some(cache.clone())).expect("a sandbox");
    refused(&sandbox, "wasi_snapshot_preview1", "fd_write");
    // Nothing was compiled: a compiled module is kept.
    assert_eq!(cached_files(&cache), Vec::<PathBuf>::new());
    // A module the emitter made is compiled, and kept where there is a disk cache (Unix only, D-120).
    sandbox
        .load(&goal("x: Number -> Number", "", &input("x")))
        .expect("loads");
    assert_eq!(cached_files(&cache).len(), usize::from(cfg!(unix)));
}

#[test]
fn ac_sec_01_only_the_whitelist_is_linked() {
    // AC-SBX-02. A name the whitelist lacks under its own module, a whitelisted name under another module, and a
    // future import of `runtime/31` §5 are all outside it.
    let sandbox = sandbox();
    for (module, name) in [
        (abi::IMPORT_MODULE, "log"),
        (abi::IMPORT_MODULE, "now"),
        (abi::IMPORT_MODULE, "call"),
        (abi::IMPORT_MODULE, "text_chars"),
        ("env", Import::NumAdd.name()),
        ("wasi_snapshot_preview1", "clock_time_get"),
        ("", ""),
    ] {
        refused(&sandbox, module, name);
    }
    // A whitelisted import with another signature is no capability, and no module either.
    let like = goal("x: Number -> Number", "", &input("x"));
    let mistyped = importing(abi::IMPORT_MODULE, Import::NumAdd.name());
    assert!(matches!(
        sandbox.load_bytes(&mistyped, &like),
        Err(LoadError::Internal(_))
    ));
    // Bytes that are no module at all never reach the scan.
    assert!(matches!(
        sandbox.load_bytes(b"\0asm", &like),
        Err(LoadError::Internal(_))
    ));
    assert_eq!(Import::ALL.len(), 14);
}

// ---- failures and limits ----

#[test]
fn ac_sbx_05_division_by_zero_is_vl0602_on_wasm() {
    let module = goal(
        "x: Number, y: Number -> Number",
        "",
        &binary("div", &input("x"), &input("y")),
    );
    let done = run(&module, &[num(1), num(0)], Limits::SYSTEM);
    let failure = done.result.expect_err("no answer");
    assert_eq!(failure.code(), Code::ArithmeticError);
    // The import's own error, with its operands (R-SBX-06).
    let op = "divide 1 by 0".to_owned();
    assert_eq!(failure.error, Error::Builtin(velme_builtins::Error::Arithmetic { op }));
    assert_eq!(
        failure.diagnostic("G", Span::new(0, 1)).message,
        "`G` tried to divide 1 by 0, which has no answer."
    );
    // The node and its two inputs.
    assert_eq!((done.spent.fuel, done.backstop), (3, None));
    assert_eq!(
        run(&module, &[num(1), num(4)], Limits::SYSTEM).result,
        Ok(Value::Number(Number::parse("0.25").expect("a number")))
    );
}

#[test]
fn r_blt_07_a_failure_names_the_items_being_visited() {
    // `map(xs, v -> map(xs, w -> v / w))`: the inner list's item first (R-SBX-19).
    let inner = collection("map", &input("xs"), "w", &binary("div", &local("v"), &local("w")));
    let body = collection("map", &input("xs"), "v", &inner);
    let module = goal("xs: List<Number> -> List<List<Number>>", "", &body);
    let xs = Value::list(vec![num(1), num(2), num(0), num(4)]);
    let failure = run(&module, &[xs], Limits::SYSTEM).result.expect_err("no answer");
    assert_eq!(failure.elements, vec![2, 0]);
    let notes = failure.diagnostic("G", Span::new(0, 1)).notes;
    assert_eq!(notes.len(), 2, "{notes:?}");
}

/// The IR and source of a goal `G` as [`goal`] makes them, for the interpreter.
fn goal_ir(signature: &str, body: &str) -> velme_ir::ValidIr {
    let (params, output) = signature.split_once("->").expect("a signature");
    let source = format!(
        "language: velme/0.1\n\ngoal G({}) -> {}:\n    plan: \"x\"\n",
        params.trim(),
        output.trim()
    );
    let inputs: Vec<String> = params
        .split(", ")
        .map(|param| {
            let (name, ty) = param.split_once(':').expect("a parameter");
            format!(r#"["{}", {}]"#, name.trim(), ty_ir(ty))
        })
        .collect();
    let ir = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "G", "types": {{}},
            "inputs": [{}], "output": {}, "body": {body}}}"#,
        inputs.join(", "),
        ty_ir(output)
    );
    valid_ir(&program(&source), &ir)
}

/// `sum(map(range(x), v -> v * v))`.
fn squares_body() -> String {
    let squared = binary("mul", &local("v"), &local("v"));
    builtin(
        "sum",
        &[&collection("map", &builtin("range", &[&input("x")]), "v", &squared)],
    )
}

fn squares() -> Module {
    goal("x: Number -> Number", "", &squares_body())
}

#[test]
fn ac_sbx_03_an_expensive_leaf_stops_with_vl0601_at_its_fuel() {
    // The figures are the interpreter's (AC-RDM-07, INV-3).
    let ir = goal_ir("x: Number -> Number", &squares_body());
    let loaded = sandbox().load(&emit(&ir).expect("emits")).expect("loads");
    let interpreted = |limits| interpret(&ir, vec![num(1000)], limits);
    let full = loaded.run(&[num(1000)], Limits::SYSTEM, &unwatched());
    let (value, spent) = interpreted(Limits::SYSTEM).expect("runs");
    assert_eq!((full.result.clone(), full.spent), (Ok(value), spent));
    for fuel in [0, 1, 2, 1000, full.spent.fuel - 1] {
        let limits = Limits { fuel, ..Limits::SYSTEM };
        let stopped = loaded.run(&[num(1000)], limits, &unwatched());
        let failure = stopped.result.expect_err("out of fuel");
        assert_eq!(Err(failure.clone()), interpreted(limits));
        assert_eq!(failure.error, Error::OutOfFuel { max_fuel: fuel });
        assert_eq!(failure.code(), Code::BudgetExceeded);
        // The deterministic limit, not the backstop behind it: all of the fuel, and no more (R-SBX-05).
        assert_eq!((stopped.spent.fuel, stopped.backstop), (fuel, None));
    }
    // Exactly enough is enough.
    let limits = Limits {
        fuel: full.spent.fuel,
        ..Limits::SYSTEM
    };
    let enough = loaded.run(&[num(1000)], limits, &unwatched());
    assert_eq!((enough.result, enough.spent), (full.result, full.spent));
    // Out of fuel inside the lambda says at which item, and out of fuel for an item's own unit is the `map`'s
    // failure (R-BLT-07), wherever the interpreter says so: here, halfway through the list.
    let stopped_at = |fuel| {
        let limits = Limits { fuel, ..Limits::SYSTEM };
        let stopped = loaded.run(&[num(1000)], limits, &unwatched());
        let failure = stopped.result.expect_err("stops");
        assert_eq!(Err(failure.clone()), interpreted(limits), "{fuel}");
        failure.elements
    };
    let middle = full.spent.fuel / 2;
    let places: Vec<Vec<usize>> = (middle..middle + 8).map(stopped_at).collect();
    assert!(places.iter().any(Vec::is_empty), "{places:?}");
    assert!(places.iter().any(|at| at.len() == 1), "{places:?}");
}

#[test]
fn ac_sbx_04_a_leaf_past_max_memory_stops_with_vl0604_at_its_memory() {
    // The figures are the interpreter's (INV-3).
    let ir = goal_ir("x: Number -> List<Number>", &builtin("range", &[&input("x")]));
    let loaded = sandbox().load(&emit(&ir).expect("emits")).expect("loads");
    let interpreted = |limits| interpret(&ir, vec![num(1000)], limits);
    let full = loaded.run(&[num(1000)], Limits::SYSTEM, &unwatched());
    let (value, spent) = interpreted(Limits::SYSTEM).expect("runs");
    assert_eq!((full.result.clone(), full.spent), (Ok(value), spent));
    for memory in [0, 8, full.spent.memory - 1] {
        let limits = Limits {
            memory,
            ..Limits::SYSTEM
        };
        let stopped = loaded.run(&[num(1000)], limits, &unwatched());
        let failure = stopped.result.expect_err("out of memory");
        assert_eq!(Err(failure.clone()), interpreted(limits));
        assert_eq!(failure.error, Error::OutOfMemory { max_memory: memory });
        assert_eq!(failure.code(), Code::MemoryLimitExceeded);
        assert_eq!((stopped.spent.memory, stopped.backstop), (memory, None));
    }
    let limits = Limits {
        memory: full.spent.memory,
        ..Limits::SYSTEM
    };
    let enough = loaded.run(&[num(1000)], limits, &unwatched());
    assert_eq!((enough.result, enough.spent), (full.result, full.spent));
    // A list past `max_list_size` is its own failure, before any memory (AC-SEC-07's list-size half).
    let too_long = loaded.run(&[num(10_001)], Limits::SYSTEM, &unwatched());
    let failure = too_long.result.expect_err("too long");
    assert_eq!(Err(failure.clone()), interpret(&ir, vec![num(10_001)], Limits::SYSTEM));
    assert_eq!(failure.code(), Code::SizeLimitExceeded);
}

#[test]
fn r_sbx_12_the_backstops_fire_behind_the_deterministic_limits() {
    let like = goal("x: Number -> Number", "", &input("x"));
    // A loop that never pays: Wasmtime's fuel ends it, as `VL0601`.
    let spinning = hostile(|f| {
        f.ops([I::Loop(Empty), I::Br(0), I::End, I::I32Const(0)]);
    });
    let limits = Limits {
        fuel: 5,
        ..Limits::SYSTEM
    };
    let spun = run_hostile(&spinning, &like, &[num(1)], limits);
    assert_eq!(spun.result, Err(Error::OutOfFuel { max_fuel: 5 }.into()));
    assert_eq!(spun.backstop, Some(Backstop::Fuel));
    // All of the backstop: `max_fuel × K + A + M × the memory limit`, with the one input `Number` of 16 bytes.
    let given = backstop_fuel(5, memory_limit(limits.memory, 16, 0));
    assert_eq!((spun.wasmtime_fuel.given, spun.wasmtime_fuel.used), (given, given));
    assert_eq!(spun.spent.fuel, 0);
    // Memory that is never charged: the resource limiter refuses it, as `VL0604` (reason 4).
    let hoarding = hostile(|f| {
        f.ops([I::Loop(Empty), I::I64Const(1 << 20), I::Call(Rt::Bump.index()), I::Drop]);
        f.ops([I::Br(0), I::End, I::I32Const(0)]);
    });
    let limits = Limits {
        memory: 1 << 20,
        ..Limits::SYSTEM
    };
    let hoarded = run_hostile(&hoarding, &like, &[num(1)], limits);
    assert_eq!(hoarded.result, Err(Error::OutOfMemory { max_memory: 1 << 20 }.into()));
    assert_eq!((hoarded.backstop, hoarded.spent.memory), (Some(Backstop::Memory), 0));
}

#[test]
fn the_memory_a_run_may_have_is_its_limit_in_pages_under_4_gib() {
    use crate::code::PAGE_BYTES;
    let limit = |max_memory, inputs, data| crate::sandbox::memory_limit(max_memory, inputs, data);
    let fixed = u64::from(abi::SCRATCH_BYTES) + (1 << 20);
    assert_eq!(limit(0, 0, 0), fixed.next_multiple_of(PAGE_BYTES));
    assert_eq!(
        limit(MAX_MEMORY, 24, 16),
        (MAX_MEMORY + 24 + 16 + fixed).next_multiple_of(PAGE_BYTES)
    );
    assert_eq!(limit(1, 0, 0) % PAGE_BYTES, 0);
    // Saturating, and one page under 4 GiB at most.
    assert_eq!(limit(u64::MAX, u32::MAX, u32::MAX), (1 << 32) - PAGE_BYTES);
    assert_eq!(limit(1 << 40, 0, 0), (1 << 32) - PAGE_BYTES);
}

// ---- R-SBX-11: a trap, and a module that lies ----

/// Sets `velme_reason` to `reason` and the fuel left to `fuel_left`.
fn claim(f: &mut Func, reason: i32, fuel_left: i64) {
    f.ops([I::I32Const(reason), I::GlobalSet(G_REASON)]);
    f.ops([I::I64Const(fuel_left), I::GlobalSet(G_FUEL)]);
}

#[test]
fn r_sbx_11_a_trap_the_module_does_not_explain_is_vl0607() {
    let like = goal("x: Number -> Number", "", &input("x"));
    let limits = Limits {
        fuel: 100,
        memory: 1 << 16,
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("unreachable with reason 0", hostile(|f| f.op(I::Unreachable))),
        (
            "reason 3",
            hostile(|f| {
                f.ops([
                    I::I32Const(Reason::Internal.code()),
                    I::Call(Rt::Fail.index()),
                    I::I32Const(0),
                ])
            }),
        ),
        (
            "out of fuel with fuel left",
            hostile(|f| {
                claim(f, Reason::OutOfFuel.code(), 1);
                f.op(I::Unreachable);
            }),
        ),
        (
            "out of memory with memory left",
            hostile(|f| {
                f.ops([
                    I::I32Const(Reason::OutOfMemory.code()),
                    I::Call(Rt::Fail.index()),
                    I::I32Const(0),
                ]);
            }),
        ),
        (
            "a reason outside 0..4",
            hostile(|f| {
                claim(f, 5, 0);
                f.op(I::Unreachable);
            }),
        ),
        (
            "a reason after a normal return",
            hostile(|f| {
                claim(f, Reason::OutOfFuel.code(), 0);
                f.op(I::I32Const(RESULT.cast_signed()));
            }),
        ),
        (
            "more fuel left than it was given",
            hostile(|f| {
                claim(f, 0, 101);
                f.op(I::I32Const(RESULT.cast_signed()));
            }),
        ),
        (
            "fuel left read as signed",
            hostile(|f| {
                claim(f, 0, -1);
                f.op(I::I32Const(RESULT.cast_signed()));
            }),
        ),
        (
            "an integer division by zero",
            hostile(|f| f.ops([I::I32Const(1), I::I32Const(0), I::I32DivU])),
        ),
        (
            "a load outside the memory",
            hostile(|f| f.ops([I::I32Const(-8), I::I32Load(mem(0, 2))])),
        ),
        ("a result outside the memory", hostile(|f| f.op(I::I32Const(-8)))),
        (
            "a result that is not the result slot",
            hostile(|f| f.op(I::I32Const(8))),
        ),
        (
            "a number that is not canonical",
            hostile(|f| {
                // A scale of 1 on the coefficient 10: 1.0, which is written 1.
                f.ops([
                    I::I32Const(RESULT.cast_signed()),
                    I::I64Const(10),
                    I::I64Store(mem(0, 3)),
                ]);
                f.ops([
                    I::I32Const(RESULT.cast_signed()),
                    I::I64Const(1 << 32),
                    I::I64Store(mem(8, 3)),
                ]);
                f.op(I::I32Const(RESULT.cast_signed()));
            }),
        ),
        (
            "an import given a number that is not canonical",
            hostile(|f| {
                f.ops([I::I64Const(10), I::I64Const(1 << 32), I::Call(Import::NumNeg.index())]);
                f.ops([I::Drop, I::Drop, I::I32Const(RESULT.cast_signed())]);
            }),
        ),
        (
            "a text written outside the marshalling bytes",
            hostile(|f| {
                f.ops([I::I64Const(1), I::I64Const(0), I::I32Const(RESULT.cast_signed())]);
                f.ops([
                    I::Call(Import::ToText.index()),
                    I::Drop,
                    I::I32Const(RESULT.cast_signed()),
                ]);
            }),
        ),
        (
            "a visiting index above the list limit",
            hostile(|f| {
                let above = i32::try_from(MAX_LIST_SIZE + 1).expect("fits");
                f.ops([I::I32Const(abi::FRAME_VISITING.cast_signed()), I::I32Const(above)]);
                f.ops([I::I32Store(mem(0, 2)), I::I64Const(0), I::GlobalSet(G_FUEL)]);
                f.ops([
                    I::I32Const(Reason::OutOfFuel.code()),
                    I::Call(Rt::Fail.index()),
                    I::I32Const(0),
                ]);
            }),
        ),
    ];
    for (what, bytes) in cases {
        let done = run_hostile(&bytes, &like, &[num(1)], limits);
        assert_eq!(done.result, internal(), "{what}");
        assert_eq!(done.backstop, None, "{what}");
    }
    // The honest forms of the same: a limit the module hit, with its global at 0.
    let honest = hostile(|f| {
        f.ops([I::I64Const(0), I::GlobalSet(G_FUEL)]);
        f.ops([
            I::I32Const(Reason::OutOfFuel.code()),
            I::Call(Rt::Fail.index()),
            I::I32Const(0),
        ]);
    });
    let done = run_hostile(&honest, &like, &[num(1)], limits);
    assert_eq!(done.result, Err(Error::OutOfFuel { max_fuel: 100 }.into()));
    assert_eq!((done.spent.fuel, done.backstop), (100, None));
}

#[test]
fn r_sbx_04_a_result_that_is_not_a_value_is_vl0607() {
    let like = goal("xs: List<Number> -> List<Number>", "", &input("xs"));
    let result = RESULT.cast_signed();
    // The list at `ptr` of `len` items, as the result: `ptr` and `len` in the result slot, and for a list the host
    // would otherwise read, the size of `len` numbers before it.
    let list = |ptr: i32, len: i32| {
        hostile(move |f| {
            if ptr >= 8 {
                let size = 16 + 16 * i64::from(len);
                f.ops([I::I32Const(ptr - 8), I::I64Const(size), I::I64Store(mem(0, 3))]);
            }
            f.ops([I::I32Const(result), I::I32Const(ptr), I::I32Store(mem(0, 2))]);
            f.ops([I::I32Const(result), I::I32Const(len), I::I32Store(mem(4, 2))]);
            f.op(I::I32Const(result));
        })
    };
    let xs = Value::list(vec![num(1), num(2)]);
    let run = |bytes: &[u8]| run_hostile(bytes, &like, std::slice::from_ref(&xs), Limits::SYSTEM).result;
    let frame = (abi::FRAME_AREA + 64).cast_signed();
    // A length above `max_list_size` whose items and size are there to read, items outside the memory, an address
    // with no room for the size before it, and a list whose size prefix is not its size.
    assert_eq!(run(&list(frame, 10_001)), internal());
    assert_eq!(run(&list(-64, 2)), internal());
    assert_eq!(run(&list(0, 0)), internal());
    let wrong = hostile(|f| {
        f.ops([I::I32Const(frame - 8), I::I64Const(16), I::I64Store(mem(0, 3))]);
        f.ops([I::I32Const(result), I::I32Const(frame), I::I32Store(mem(0, 2))]);
        f.ops([I::I32Const(result), I::I32Const(1), I::I32Store(mem(4, 2))]);
        f.op(I::I32Const(result));
    });
    assert_eq!(run(&wrong), internal());
    // At `max_list_size`, the same list is read.
    assert_eq!(run(&list(frame, 10_000)), Ok(Value::list(vec![num(0); 10_000])));
    // The input list itself, which the host wrote, is one: the first input's slot holds its address and length.
    let echo = hostile(|f| {
        f.ops([
            I::I32Const(result),
            I::LocalGet(0),
            I::I64Load(mem(0, 3)),
            I::I64Store(mem(0, 3)),
        ]);
        f.op(I::I32Const(result));
    });
    assert_eq!(run(&echo), Ok(xs.clone()));
}

#[test]
fn r_sbx_11_decoding_stops_at_what_a_run_could_have_made() {
    // A list of `max_list_size` lists, every one of them the input list: far more than the run was charged for or
    // given, so the host stops reading (R-SBX-11). The memory claims the outer list's true size, so only the
    // budget stops it.
    let body = format!(
        r#"{{"kind": "list", "of": {{"t": "List", "of": {{"t": "Number"}}}}, "items": [{}]}}"#,
        input("xs")
    );
    let like = goal("xs: List<Number> -> List<List<Number>>", "", &body);
    let result = RESULT.cast_signed();
    let area = (abi::frame(1) + abi::FRAME_AREA).cast_signed();
    let count = i32::try_from(MAX_LIST_SIZE).expect("fits");
    let aliased = hostile(|f| {
        let i = f.local(V::I32);
        f.ops([I::I32Const(0), I::LocalSet(i), I::Block(Empty), I::Loop(Empty)]);
        f.ops([I::LocalGet(i), I::I32Const(count), I::I32GeU, I::BrIf(1)]);
        f.ops([I::I32Const(area), I::LocalGet(i), I::I32Const(8), I::I32Mul, I::I32Add]);
        f.ops([I::LocalGet(0), I::I64Load(mem(0, 3)), I::I64Store(mem(0, 3))]);
        f.ops([
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let size = 16 + i64::from(count) * (16 + 16 * 1000);
        f.ops([I::I32Const(area - 8), I::I64Const(size), I::I64Store(mem(0, 3))]);
        f.ops([I::I32Const(result), I::I32Const(area), I::I32Store(mem(0, 2))]);
        f.ops([I::I32Const(result), I::I32Const(count), I::I32Store(mem(4, 2))]);
        f.op(I::I32Const(result));
    });
    let xs = Value::list((0..1000).map(num).collect());
    let limits = Limits {
        fuel: MAX_FUEL,
        memory: 1 << 16,
    };
    assert_eq!(run_hostile(&aliased, &like, &[xs], limits).result, internal());
}

#[test]
fn r_sbx_11_lists_of_nothing_are_read_back_only_as_far_as_a_run_paid_for_them() {
    // `max_list_size` lists of `Nothing` of every length up to it: 16 bytes each, but 5·10⁷ items for the host to
    // make, and the module paid for none. Each length is made once, and only as far as the fuel spent, the inputs'
    // and the literals' lists of `Nothing` go (R-SBX-11, D-120).
    let like = goal(
        "x: Number -> List<List<Nothing>>",
        "",
        r#"{"kind": "list", "of": {"t": "List", "of": {"t": "Nothing"}}, "items": []}"#,
    );
    let result = RESULT.cast_signed();
    let area = (abi::frame(1) + abi::FRAME_AREA).cast_signed();
    let shared = result + 64;
    let count = i32::try_from(MAX_LIST_SIZE).expect("fits");
    let lengths = hostile(|f| {
        let i = f.local(V::I32);
        f.ops([I::I32Const(shared - 8), I::I64Const(16), I::I64Store(mem(0, 3))]);
        f.ops([I::I32Const(0), I::LocalSet(i), I::Block(Empty), I::Loop(Empty)]);
        f.ops([I::LocalGet(i), I::I32Const(count), I::I32GeU, I::BrIf(1)]);
        f.ops([I::I32Const(area), I::LocalGet(i), I::I32Const(8), I::I32Mul, I::I32Add]);
        f.ops([I::I32Const(shared), I::I32Store(mem(0, 2))]);
        f.ops([I::I32Const(area), I::LocalGet(i), I::I32Const(8), I::I32Mul, I::I32Add]);
        f.ops([I::LocalGet(i), I::I32Store(mem(4, 2))]);
        f.ops([
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let size = 16 + 16 * i64::from(count);
        f.ops([I::I32Const(area - 8), I::I64Const(size), I::I64Store(mem(0, 3))]);
        f.ops([I::I32Const(result), I::I32Const(area), I::I32Store(mem(0, 2))]);
        f.ops([I::I32Const(result), I::I32Const(count), I::I32Store(mem(4, 2))]);
        f.op(I::I32Const(result));
    });
    let limits = Limits {
        fuel: MAX_FUEL,
        memory: 1 << 20,
    };
    assert_eq!(run_hostile(&lengths, &like, &[num(1)], limits).result, internal());

    // A run that makes such lists pays for their items, and gets them back: here as the interpreter does.
    let nothing = r#"{"kind": "literal", "type": {"t": "Nothing"}, "value": null}"#;
    let inner = collection("map", &builtin("range", &[&local("i")]), "v", nothing);
    let body = collection("map", &builtin("range", &[&input("x")]), "i", &inner);
    let ir = goal_ir("x: Number -> List<List<Nothing>>", &body);
    let loaded = sandbox().load(&emit(&ir).expect("emits")).expect("loads");
    let made = loaded.run(&[num(200)], Limits::SYSTEM, &unwatched());
    let (value, spent) = interpret(&ir, vec![num(200)], Limits::SYSTEM).expect("runs");
    assert_eq!((made.result, made.spent), (Ok(value), spent));
    // So does one that returns its input's, or a literal's, which it did not pay for.
    let echo = goal_ir("xs: List<Nothing> -> List<Nothing>", &input("xs"));
    let xs = Value::list(vec![Value::Nothing; 5000]);
    let echoed = run(&emit(&echo).expect("emits"), std::slice::from_ref(&xs), Limits::SYSTEM);
    assert_eq!(echoed.result, Ok(xs));
    let literal = r#"{"kind": "literal", "type": {"t": "List", "of": {"t": "Nothing"}}, "value": [null, null, null]}"#;
    let literal = goal_ir("x: Number -> List<Nothing>", literal);
    let constant = run(&emit(&literal).expect("emits"), &[num(1)], Limits::SYSTEM);
    assert_eq!(constant.result, Ok(Value::list(vec![Value::Nothing; 3])));
}

#[test]
fn a_module_that_uses_all_its_stack_is_vl0607_and_nothing_worse() {
    // `runtime/31` §6: the WASM stack is 4 MiB on every host, and running out of it is a trap, not the end of the
    // thread's own stack. Wasmtime's fuel would stop it later.
    let mut assembly = Assembly::default();
    runtime::define(&mut assembly);
    let run = assembly.reserve();
    let mut f = Func::new(&[V::I32], &[V::I32]);
    f.ops([I::LocalGet(0), I::Call(run)]);
    assembly.define(run, f);
    let bytes = assembly.finish(&[], Rt::Alloc.index(), run).expect("a module");
    let like = goal("x: Number -> Number", "", &input("x"));
    let limits = Limits {
        fuel: MAX_FUEL,
        ..Limits::SYSTEM
    };
    let done = run_hostile(&bytes, &like, &[num(1)], limits);
    assert_eq!((done.result, done.backstop), (internal(), None));
}

#[test]
fn the_deepest_body_the_validator_allows_runs_within_the_stack() {
    // `runtime/31` §6: expressions at `MAX_DEPTH`, and collection nodes at `MAX_COLLECTION_NESTING` inside them.
    let mut deep = number("1");
    for _ in 0..MAX_DEPTH - 3 {
        deep = binary("add", &number("1"), &deep);
    }
    let module = emit(&valid_ir(&program(EVERYTHING), &document(&item(&deep, &input("s"))))).expect("emits");
    let inputs = [
        Value::list(vec![]),
        Value::list(vec![]),
        Value::text("s"),
        Value::Nothing,
    ];
    let Ok(Value::Record(record)) = run(&module, &inputs, Limits::SYSTEM).result else {
        panic!("a record");
    };
    assert_eq!(record.get("k"), Some(&num(i64::try_from(MAX_DEPTH - 2).expect("fits"))));

    // `sum(map(xs, a -> sum(map(xs, b -> … a + b …))))`, one `map` in the lambda of the last, as deep as they go.
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let depth = MAX_COLLECTION_NESTING;
    let mut body = binary("add", &local(names[depth - 1]), &local(names[0]));
    for name in names[..depth].iter().rev() {
        body = builtin("sum", &[&collection("map", &input("xs"), name, &body)]);
    }
    let module = goal("xs: List<Number> -> Number", "", &body);
    let xs = Value::list(vec![num(1), num(2), num(3)]);
    let expected = 3i64.pow(u32::try_from(depth).expect("fits") - 1) * 6 * 2;
    assert_eq!(run(&module, &[xs], Limits::SYSTEM).result, Ok(num(expected)));
}

// ---- the fuel backstop ----

#[test]
fn the_fuel_backstop_covers_every_runtime_function() {
    // D-115: K is 4 × the most instructions one paid unit covers, and A is 4 × what runs unpaid. A body's length
    // bounds what it runs between two turns of a loop in it, so a body that outgrows what the constants assume
    // fails here.
    let mut total = 0;
    for rt in Rt::ALL {
        let body = rt.instructions();
        total += body;
        match rt {
            // A unit pays for a block of bytes, one turn of the loop each.
            Rt::TextEq | Rt::TextChars => assert!(body <= BYTE_INSTRUCTIONS, "{rt:?}: {body}"),
            // Paid after its work, a unit for each item.
            Rt::Sum => assert!(body <= SUM_ITEM_INSTRUCTIONS, "{rt:?}: {body}"),
            // `n·⌈log2(n+1)⌉` units for `n` turns to number the items, at most `⌈log2 n⌉` passes, and in each pass
            // at most `n` runs and `n` items merged: under three turns a unit.
            Rt::Sort => assert!(3 * body <= UNIT_INSTRUCTIONS, "{rt:?}: {body}"),
            // A unit an item: one turn each.
            Rt::Range | Rt::Extreme => assert!(body <= UNIT_INSTRUCTIONS, "{rt:?}: {body}"),
            // No loop: run once for a node, an element or a pair that has paid its unit.
            Rt::Fail
            | Rt::Tick
            | Rt::ChargeFuel
            | Rt::ChargeMemory
            | Rt::Afford
            | Rt::Bump
            | Rt::SatAdd
            | Rt::TextBytes
            | Rt::Blocks
            | Rt::Concat
            | Rt::ToText
            | Rt::Alloc => {}
        }
    }
    // Every one of them together, once, is within one unit's instructions: what a node's own unit covers.
    assert!(total <= UNIT_INSTRUCTIONS, "{total}");
    assert_eq!(UNIT_INSTRUCTIONS, TEXT_BLOCK_BYTES * BYTE_INSTRUCTIONS);
    assert_eq!(FUEL_FACTOR, 4 * UNIT_INSTRUCTIONS);
    assert_eq!(FUEL_ALLOWANCE, 4 * (MAX_LIST_SIZE * SUM_ITEM_INSTRUCTIONS + 1024));
    // D-120: M is 4 × the times one charged byte is moved by a bulk copy, a `map` result's twice.
    assert_eq!(MEMORY_MOVES, 2);
    assert_eq!(MEMORY_FACTOR, 4 * MEMORY_MOVES);
    assert_eq!(backstop_fuel(0, 0), FUEL_ALLOWANCE);
    assert_eq!(
        backstop_fuel(10, 1 << 16),
        10 * FUEL_FACTOR + FUEL_ALLOWANCE + MEMORY_FACTOR * (1 << 16)
    );
    assert_eq!(backstop_fuel(u64::MAX, 0), u64::MAX);
    assert_eq!(backstop_fuel(0, u64::MAX), u64::MAX);
}

#[test]
fn the_fuel_backstop_counts_every_byte_a_bulk_copy_moves() {
    // Wasmtime charges a unit of its fuel per byte `memory.copy` moves, and checks its fuel and the epoch after it
    // (D-120): a hundred copies of a page, paid for by nothing, use at least that many units.
    let like = goal("x: Number -> Number", "", &input("x"));
    let page = i32::try_from(crate::code::PAGE_BYTES).expect("fits");
    let copies = hostile(|f| {
        for _ in 0..100 {
            f.ops([
                I::I32Const(0),
                I::I32Const(page),
                I::I32Const(page),
                crate::runtime::COPY,
            ]);
        }
        f.ops([
            I::I32Const(1),
            I::GlobalSet(G_REASON),
            I::I64Const(0),
            I::GlobalSet(G_FUEL),
        ]);
        f.op(I::Unreachable);
        f.op(I::I32Const(0));
    });
    let run = run_hostile(&copies, &like, &[num(1)], Limits::SYSTEM);
    assert_eq!(run.result, Err(Error::OutOfFuel { max_fuel: MAX_FUEL }.into()));
    assert!(run.wasmtime_fuel.used >= 100 * crate::code::PAGE_BYTES, "{run:?}");
}

#[test]
fn the_costliest_units_stay_within_a_quarter_of_the_backstop() {
    // The work one unit pays for at its most: a text compared a byte at a time, the characters of a text counted,
    // a full list summed before it is paid, and a full list sorted (R-SBX-15's bound, where it is tightest).
    let sandbox = sandbox();
    let quarter = |module: &Module, inputs: &[Value]| {
        let run = sandbox
            .load(module)
            .expect("loads")
            .run(inputs, Limits::SYSTEM, &unwatched());
        assert!(run.result.is_ok(), "{run:?}");
        assert_eq!(run.backstop, None);
        assert!(run.wasmtime_fuel.within_a_quarter(), "{run:?}");
        run
    };
    let long = Value::text(&"ab".repeat(30_000));
    let texts = goal(
        "x: Text, y: Text -> Boolean",
        "",
        &binary("eq", &input("x"), &input("y")),
    );
    quarter(&texts, &[long.clone(), long.clone()]);
    let chars = goal("x: Text -> Number", "", &builtin("length", &[&input("x")]));
    quarter(&chars, &[long]);
    let full = Value::list((0..10_000).map(num).collect());
    let sum = goal("xs: List<Number> -> Number", "", &builtin("sum", &[&input("xs")]));
    // A sum is charged after its work, so with no fuel it is all unpaid: what A allows for (D-115).
    let unpaid = sandbox.load(&sum).expect("loads").run(
        std::slice::from_ref(&full),
        Limits {
            fuel: 1,
            ..Limits::SYSTEM
        },
        &unwatched(),
    );
    assert_eq!(unpaid.result.as_ref().expect_err("stops").code(), Code::BudgetExceeded);
    assert_eq!(unpaid.backstop, None);
    assert!(unpaid.wasmtime_fuel.within_a_quarter(), "{unpaid:?}");
    quarter(&sum, std::slice::from_ref(&full));
    let key = binary("sub", &number("0"), &local("v"));
    let sort = format!(
        r#"{{"kind": "sort_by", "list": {}, "key": {{"param": "v", "body": {key}}}, "descending": false}}"#,
        input("xs")
    );
    quarter(&goal("xs: List<Number> -> List<Number>", "", &sort), &[full]);
}

/// A goal `G(xs: List<W>, a: W, b: W)` giving `output` by `body`, where the record `W` has `width` numbers.
fn wide_goal(width: usize, output: &str, body: &str) -> velme_ir::ValidIr {
    let fields: Vec<String> = (0..width).map(|i| format!("f{i}")).collect();
    let declared: String = fields.iter().map(|f| format!("    {f}: Number\n")).collect();
    let source = format!(
        "language: velme/0.1\n\ntype W:\n{declared}\ngoal G(xs: List<W>, a: W, b: W) -> {output}:\n    plan: \"x\"\n"
    );
    let listed: Vec<String> = fields
        .iter()
        .map(|f| format!(r#"["{f}", {{"t": "Number"}}]"#))
        .collect();
    let w = r#"{"t": "Record", "name": "W"}"#;
    let ir = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "G",
            "types": {{"W": {{"fields": [{}]}}}},
            "inputs": [["xs", {{"t": "List", "of": {w}}}], ["a", {w}], ["b", {w}]], "output": {}, "body": {body}}}"#,
        listed.join(", "),
        ty_ir(output)
    );
    valid_ir(&program(&source), &ir)
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(16))]

    /// Very wide records compared, sorted, filtered and rebuilt: every byte they take is copied, and Wasmtime charges
    /// each (D-120), yet no run comes near a quarter of its backstop, and each gives what the interpreter gives.
    #[test]
    fn r_sbx_15_wide_records_stay_within_a_quarter_of_the_backstop(
        width in 1usize..=256,
        len in 0usize..=40,
        seed in 0i64..1000,
        same in proptest::bool::ANY,
    ) {
        let record = |shift: i64| {
            let fields = (0..width).map(|i| {
                let i = i64::try_from(i).expect("fits");
                (format!("f{i}"), num((seed + shift + 7 * i) % 5 - 2))
            });
            Value::record("W", fields.collect())
        };
        let xs = Value::list((0..len).map(|j| record(i64::try_from(j).expect("fits") * 31)).collect());
        let a = record(0);
        let b = if same { record(0) } else { record(1) };
        let w = |name: &str| field(&local("w"), name);
        let last = format!("f{}", width - 1);
        let rebuilt = format!(
            r#"{{"kind": "record", "type": "W", "fields": {{{}}}}}"#,
            (0..width).map(|i| format!(r#""f{i}": {}"#, w(&format!("f{i}")))).collect::<Vec<_>>().join(", ")
        );
        let sorted = format!(
            r#"{{"kind": "sort_by", "list": {}, "key": {{"param": "w", "body": {}}}, "descending": true}}"#,
            input("xs"),
            w(&last)
        );
        let equal = binary("and", &binary("eq", &input("a"), &input("b")), &binary("eq", &input("xs"), &input("xs")));
        for (output, body) in [
            ("Boolean", equal),
            ("List<W>", sorted),
            ("List<W>", collection("filter", &input("xs"), "w", &binary("ge", &w("f0"), &number("0")))),
            ("List<W>", collection("map", &input("xs"), "w", &rebuilt)),
        ] {
            let ir = wide_goal(width, output, &body);
            let inputs = vec![xs.clone(), a.clone(), b.clone()];
            let run = run(&emit(&ir).expect("emits"), &inputs, Limits::SYSTEM);
            proptest::prop_assert_eq!(run.backstop, None);
            proptest::prop_assert!(run.wasmtime_fuel.within_a_quarter(), "{:?}", run.wasmtime_fuel);
            let interpreted = interpret(&ir, inputs, Limits::SYSTEM);
            proptest::prop_assert_eq!(run.result.map(|value| (value, run.spent)), interpreted);
        }
    }
}

// ---- the watchdog ----

#[test]
fn ac_sbx_08_a_run_past_its_wall_clock_is_vl0603() {
    // The injected clock: a number the test moves, and a watchdog that reads it (D-51, D-115).
    let now = Arc::new(AtomicU64::new(0));
    let max_wall_clock = 1_000;
    let clock = Arc::clone(&now);
    let watchdog = Interrupt::new(move || clock.load(Ordering::SeqCst) > max_wall_clock);
    let loaded = sandbox().load(&squares()).expect("loads");
    let in_time = loaded.run(&[num(100)], Limits::SYSTEM, &watchdog);
    assert_eq!(in_time.result, Ok(num(328_350)));
    now.store(max_wall_clock + 1, Ordering::SeqCst);
    let late = loaded.run(&[num(100)], Limits::SYSTEM, &watchdog);
    let failure = late.result.expect_err("stopped");
    // `VL0603`, the one outcome that is not reproducible (D-10): no limit of the run's own was hit.
    assert_eq!(failure.error, Error::Interrupted);
    assert_eq!(failure.code(), Code::Timeout);
    assert_eq!(late.backstop, None);
    assert!(late.spent.fuel < in_time.spent.fuel);
}

#[test]
fn ac_sbx_08_the_watchdog_is_asked_at_every_tick_until_it_stops_the_module() {
    // A module that never ends and never pays, with fuel without end: only the watchdog stops it. It is asked when
    // the run starts and then at each tick of the engine's own ticker; here it gives in the third time.
    let like = goal("x: Number -> Number", "", &input("x"));
    let spinning = hostile(|f| f.ops([I::Loop(Empty), I::Br(0), I::End, I::I32Const(0)]));
    let asked = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&asked);
    let watchdog = Interrupt::new(move || count.fetch_add(1, Ordering::SeqCst) >= 2);
    let limits = Limits {
        fuel: u64::MAX,
        ..Limits::SYSTEM
    };
    let program = sandbox().load_bytes(&spinning, &like).expect("loads");
    let stopped = program.run(&[num(1)], limits, &watchdog);
    assert_eq!(stopped.result, Err(Error::Interrupted.into()));
    assert_eq!(stopped.backstop, None);
    assert_eq!(asked.load(Ordering::SeqCst), 3);
}

// ---- the cache ----

#[cfg(unix)]
#[test]
fn ac_sbx_07_deleting_the_cache_changes_nothing_and_the_file_comes_back() {
    let scratch = Scratch::new("cache");
    let cache = scratch.0.join("velme").join("wasm");
    let sandbox = Sandbox::new(Some(cache.clone())).expect("a sandbox");
    let module = squares();
    let once = |sandbox: &Sandbox| {
        sandbox
            .load(&module)
            .expect("loads")
            .run(&[num(100)], Limits::SYSTEM, &unwatched())
    };
    // The next run is another process, with a sandbox of its own: one sandbox compiles a module once.
    let next = || Sandbox::new(Some(cache.clone())).expect("a sandbox");
    let first = once(&sandbox);
    assert_eq!(first.result, Ok(num(328_350)));
    let files = cached_files(&cache);
    assert_eq!(files.len(), 1, "{files:?}");
    // `<BLAKE3 of the module>-<compatibility hash>.cwasm` (R-SBX-13), and nothing temporary left behind.
    let name = files[0].file_name().and_then(|n| n.to_str()).expect("a name");
    let hash = blake3::hash(module.bytes()).to_hex();
    assert!(
        name.starts_with(&format!("{hash}-")) && name.ends_with(".cwasm"),
        "{name}"
    );
    assert_eq!(name.len(), 64 + 1 + 16 + ".cwasm".len());
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &Path| std::fs::metadata(path).expect("there").permissions().mode() & 0o777;
        assert_eq!(mode(&cache), 0o700);
        assert_eq!(mode(&files[0]), 0o600);
    }
    // Loaded from the file the second time: it is not written again.
    let written = std::fs::metadata(&files[0])
        .expect("the file")
        .modified()
        .expect("a time");
    assert_eq!(once(&sandbox), first);
    assert_eq!(once(&next()), first);
    assert_eq!(
        std::fs::metadata(&files[0])
            .expect("the file")
            .modified()
            .expect("a time"),
        written
    );

    std::fs::remove_dir_all(&cache).expect("deletes");
    assert_eq!(once(&next()), first);
    assert_eq!(cached_files(&cache), files);

    // A file that does not load is deleted and the module compiled again (R-SBX-14).
    std::fs::write(&files[0], b"not a compiled module").expect("writes");
    assert_eq!(once(&next()), first);
    assert_eq!(cached_files(&cache), files);
    assert_ne!(std::fs::read(&files[0]).expect("reads"), b"not a compiled module");
    // Another module is another file.
    sandbox
        .load(&goal("x: Number -> Number", "", &input("x")))
        .expect("loads");
    assert_eq!(cached_files(&cache).len(), 2);
    // With no directory, nothing is written anywhere.
    assert_eq!(once(&Sandbox::new(None).expect("a sandbox")), first);
    assert_eq!(cached_files(&cache).len(), 2);
    assert_eq!(sandbox.cache_off(), None);
}

/// Off Unix there is no disk cache in v0.1 (D-120): a directory given is never written, and the runtime can say so.
#[cfg(not(unix))]
#[test]
fn ac_sbx_07_there_is_no_disk_cache_off_unix() {
    let scratch = Scratch::new("cache");
    let cache = scratch.0.join("velme").join("wasm");
    let sandbox = Sandbox::new(Some(cache.clone())).expect("a sandbox");
    let run = sandbox
        .load(&squares())
        .expect("loads")
        .run(&[num(100)], Limits::SYSTEM, &unwatched());
    assert_eq!(run.result, Ok(num(328_350)));
    assert_eq!(cached_files(&cache), Vec::<PathBuf>::new());
    assert!(sandbox.cache_off().is_some());
}

#[test]
fn ac_sbx_09_a_cwasm_under_the_project_is_never_read() {
    // T-11: a cloned project ships compiled modules where a cache might be looked for, under the very name the
    // cache would use. The cache is the user's directory, and only a file directly in it is ever loaded.
    let scratch = Scratch::new("planted");
    let user = scratch.0.join("user-cache").join("velme").join("wasm");
    let project = scratch.0.join("project");
    let sandbox = Sandbox::new(Some(user.clone())).expect("a sandbox");
    let module = squares();
    let cache = sandbox.cache().expect("a cache");
    let name = cache.path(module.bytes());
    let name = name.file_name().expect("a name");
    let garbage = b"\x7fELF not a module Velme compiled".as_slice();
    let mut planted = Vec::new();
    for dir in [
        project.join(".velme"),
        project.join(".velme").join("cache").join("wasm"),
    ] {
        std::fs::create_dir_all(&dir).expect("a directory");
        for file in [dir.join(name), dir.join("module.cwasm")] {
            std::fs::write(&file, garbage).expect("plants");
            planted.push(file);
        }
    }
    // The goal compiles from its IR and runs, from inside the project too.
    let done = sandbox
        .load(&module)
        .expect("loads")
        .run(&[num(100)], Limits::SYSTEM, &unwatched());
    assert_eq!(done.result, Ok(num(328_350)));

    // The loader refuses every path outside the configured directory before it touches the disk: the files are
    // neither loaded nor deleted as ones that failed to load.
    let outside = [
        planted.clone(),
        vec![
            user.join("deeper").join(name),
            user.join("..").join("wasm").join(name),
            user.join(name).join(".."),
            user.with_file_name("other").join(name),
            user.join("module.json"),
            user.clone(),
            PathBuf::from(name),
        ],
    ]
    .concat();
    for path in &outside {
        assert_eq!(cache.read(path), Err(Refused::Outside), "{}", path.display());
    }
    #[cfg(unix)]
    {
        let real = cached_files(&user).remove(0);
        assert_eq!(real.file_name(), Some(name));
        assert!(cache.read(&real).expect("reads").is_some());
        assert_eq!(cache.read(&user.join("absent.cwasm")), Ok(None));
    }
    for file in &planted {
        assert_eq!(std::fs::read(file).expect("still there"), garbage, "{}", file.display());
    }
    assert_eq!(cached_files(&project.join(".velme")).len(), 3);
}

/// R-SBX-20: a directory that is a symbolic link, or open to its group or to others, is refused on its open handle
/// and never changed; a file that is a symbolic link is refused. Either turns the disk cache off for the process,
/// and the goal still runs, compiled.
#[cfg(unix)]
#[test]
fn r_sbx_20_a_cache_that_is_not_the_users_alone_is_not_used() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let scratch = Scratch::new("refused");
    let module = squares();
    let expected = Ok(num(328_350));
    let runs = |sandbox: &Sandbox| {
        let run = sandbox.load(&module).expect("loads");
        run.run(&[num(100)], Limits::SYSTEM, &unwatched()).result
    };
    let mode = |path: &Path| std::fs::symlink_metadata(path).expect("there").permissions().mode() & 0o777;

    // A file in the cache that is a symbolic link, to a planted module under the very name the cache uses.
    let user = scratch.0.join("user");
    let sandbox = Sandbox::new(Some(user.clone())).expect("a sandbox");
    let cache = sandbox.cache().expect("a cache");
    let planted = scratch.0.join("planted.cwasm");
    std::fs::write(&planted, b"not a module").expect("plants");
    std::fs::create_dir_all(&user).expect("a directory");
    std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o700)).expect("closes");
    let path = cache.path(module.bytes());
    symlink(&planted, &path).expect("links");
    assert_eq!(cache.read(&path), Err(Refused::Unusable(Unusable::File)));
    assert_eq!(runs(&sandbox), expected);
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("there")
            .file_type()
            .is_symlink()
    );
    assert!(sandbox.cache_off().is_some_and(|note| note.contains("symbolic link")));
    // Off for the process: the link gone, the cache is still not used.
    std::fs::remove_file(&path).expect("unlinks");
    assert_eq!(runs(&sandbox), expected);
    assert_eq!(cached_files(&user), Vec::<PathBuf>::new());

    // A directory that is a symbolic link, to one that would pass.
    let target = scratch.0.join("target");
    std::fs::create_dir_all(&target).expect("a directory");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("closes");
    let link = scratch.0.join("link");
    symlink(&target, &link).expect("links");
    let linked = Sandbox::new(Some(link.clone())).expect("a sandbox");
    let through = linked.cache().expect("a cache");
    let name = path.file_name().expect("a name");
    assert_eq!(
        through.read(&link.join(name)),
        Err(Refused::Unusable(Unusable::Directory))
    );
    assert_eq!(runs(&linked), expected);
    assert_eq!(cached_files(&target), Vec::<PathBuf>::new());

    // A directory its group, or others, can reach: refused, and left as it is.
    for open in [0o777, 0o750, 0o710, 0o701] {
        let dir = scratch.0.join(format!("open-{open:o}"));
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(open)).expect("opens");
        let opened = Sandbox::new(Some(dir.clone())).expect("a sandbox");
        let cache = opened.cache().expect("a cache");
        assert_eq!(cache.read(&dir.join(name)), Err(Refused::Unusable(Unusable::Open)));
        assert_eq!(runs(&opened), expected);
        assert_eq!(cached_files(&dir), Vec::<PathBuf>::new());
        assert_eq!(mode(&dir), open);
        assert!(
            opened
                .cache_off()
                .is_some_and(|note| note.contains(&dir.display().to_string()))
        );
    }
}

/// R-SBX-20's owner rule, which a test run by one user cannot set up on disk: what `fstat` of the open handle says
/// decides.
#[cfg(unix)]
#[test]
fn r_sbx_20_the_owner_and_the_mode_decide_on_the_handle() {
    use rustix::fs::{FileType, Mode};
    let me = 501;
    let dir = |mode, owner| crate::cache::verdict(FileType::Directory, Mode::from_bits_truncate(mode), owner, me, true);
    let file =
        |mode, owner| crate::cache::verdict(FileType::RegularFile, Mode::from_bits_truncate(mode), owner, me, false);
    assert_eq!(dir(0o700, me), Ok(()));
    assert_eq!(dir(0o500, me), Ok(()));
    assert_eq!(dir(0o700, 0), Err(Unusable::Owner));
    assert_eq!(dir(0o700, 502), Err(Unusable::Owner));
    for open in [0o770, 0o707, 0o710, 0o701, 0o740, 0o704] {
        assert_eq!(dir(open, me), Err(Unusable::Open), "{open:o}");
    }
    let not_a_dir = crate::cache::verdict(FileType::Symlink, Mode::from_bits_truncate(0o700), me, me, true);
    assert_eq!(not_a_dir, Err(Unusable::Directory));
    assert_eq!(file(0o600, me), Ok(()));
    assert_eq!(file(0o644, me), Ok(()));
    assert_eq!(file(0o600, 0), Err(Unusable::File));
    assert_eq!(file(0o620, me), Err(Unusable::File));
    assert_eq!(file(0o602, me), Err(Unusable::File));
    for kind in [FileType::Symlink, FileType::Fifo, FileType::Directory] {
        let wrong = crate::cache::verdict(kind, Mode::from_bits_truncate(0o600), me, me, false);
        assert_eq!(wrong, Err(Unusable::File), "{kind:?}");
    }
}

/// A loaded module is its own: programs and the sandbox can be used from any thread.
#[test]
fn a_program_runs_from_many_threads_at_once() {
    fn shared<T: Send + Sync>() {}
    shared::<Sandbox>();
    shared::<Program>();
    let loaded = sandbox().load(&squares()).expect("loads");
    let first = loaded.run(&[num(100)], Limits::SYSTEM, &unwatched());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| assert_eq!(loaded.run(&[num(100)], Limits::SYSTEM, &unwatched()), first));
        }
    });
}
