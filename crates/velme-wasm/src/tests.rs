//! The emitter on the golden IR and the examples (`runtime/31` §4, §10): every leaf goal becomes a module that
//! validates under the R-SBX-07 feature set and has the R-SBX-03 interface. What a module computes is the
//! differential suite's to prove (R-SBX-15); nothing here runs one.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use velme_builtins::limits::MAX_LIST_SIZE;
use velme_builtins::memory::{list_bytes, value_bytes};
use velme_builtins::{BUILTINS_VERSION, CATALOG, Number, Value};
use velme_ir::{Goal, IR_VERSION, Node, ValidIr, calls, from_json_str, to_canonical_string, wired_goal};
use velme_test_support::{goal_id, program, read, repo, valid_ir};
use wasm_encoder::{
    CodeSection, Function, FunctionSection, HeapType, Instruction, MemorySection, MemoryType, TagKind, TagSection,
    TagType, TypeSection, ValType,
};
use wasmparser::{Parser, Payload, Validator, WasmFeatures};

use crate::abi::{self, Import, LIST_PREFIX, Reason};
use crate::ty::{Ty, Types};
use crate::validate::{FEATURES, validate};
use crate::{EmitError, emit};

/// The examples, each with where the hand-written IR of its goals is: a directory of `<Goal>.json`, or one file.
const EXAMPLES: [(&str, &str); 7] = [
    ("examples/beginner/add.velme", "tests/fixtures/run/add.json"),
    ("examples/beginner/hello.velme", "tests/fixtures/run/hello.ir"),
    ("examples/beginner/find_badge.velme", "tests/fixtures/run/find_badge.ir"),
    (
        "examples/beginner/double_then_add_one.velme",
        "tests/fixtures/run/double_then_add_one.ir",
    ),
    (
        "examples/intermediate/player_summary.velme",
        "tests/fixtures/run/player_summary.ir",
    ),
    (
        "examples/games/level_summary.velme",
        "tests/fixtures/run/level_summary.ir",
    ),
    (
        "examples/professional/order_total.velme",
        "tests/fixtures/run/order_total.ir",
    ),
];

/// The `.json` files of `dir`, or `dir` itself if it is one, in name order.
fn documents(dir: &Path) -> Vec<PathBuf> {
    if dir.is_file() {
        return vec![dir.to_owned()];
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

/// The IR of every leaf goal of the golden set and of the examples, validated, with a name for messages. A composite
/// goal's tail stays on the interpreter (R-SBX-17).
fn corpus() -> Vec<(String, ValidIr)> {
    let mut sources = vec![("tests/golden/ir/goals.velme", "tests/golden/ir/accept")];
    sources.extend(EXAMPLES);
    let mut out = Vec::new();
    for (source, ir) in sources {
        let program = program(&read(&repo(source)));
        for path in documents(&repo(ir)) {
            let text = read(&path);
            let goal: Goal = from_json_str(&text).expect("an IR document");
            let calls = calls(&program, goal_id(&program, &goal.goal)).expect("a checked goal");
            if calls.is_empty() {
                out.push((path.display().to_string(), valid_ir(&program, &text)));
            }
        }
    }
    out
}

/// What a module imports, as `module.name`, and exports, in section order.
fn interface(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let (mut imports, mut exports) = (Vec::new(), Vec::new());
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("a module") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("an import");
                    imports.push(format!("{}.{}", import.module, import.name));
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    exports.push(export.expect("an export").name.to_owned());
                }
            }
            _ => {}
        }
    }
    (imports, exports)
}

/// A module that validates, with the exports of R-SBX-03 and the imports of `runtime/31` §5, and nothing else.
fn assert_module(name: &str, ir: &ValidIr) -> Vec<u8> {
    let module = emit(ir).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    // `emit` has validated it; this is the same check, where a failure says which goal.
    validate(module.bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (imports, exports) = interface(module.bytes());
    let whitelist: Vec<String> = Import::ALL
        .iter()
        .map(|i| format!("{}.{}", abi::IMPORT_MODULE, i.name()))
        .collect();
    assert_eq!(imports, whitelist, "{name}");
    let expected = [
        abi::MEMORY,
        abi::ALLOC,
        abi::RUN,
        abi::FUEL_LEFT,
        abi::MEMORY_LEFT,
        abi::REASON,
    ];
    assert_eq!(exports, expected, "{name}");
    module.bytes().to_vec()
}

#[test]
fn ac_sbx_06_every_golden_and_example_leaf_validates() {
    let corpus = corpus();
    // The three golden leaves and the examples' fifteen.
    assert!(corpus.len() >= 18, "{} leaf goals", corpus.len());
    for (name, ir) in &corpus {
        assert_module(name, ir);
    }
}

#[test]
fn emission_is_deterministic() {
    // The same IR read twice gives the same bytes: a module is a pure function of its IR (R-SBX-03, CC-DET-01).
    for ((name, first), (_, second)) in corpus().iter().zip(&corpus()) {
        assert_eq!(emit(first), emit(second), "{name}");
    }
    let (program, document) = (program(EVERYTHING), everything());
    let (first, second) = (valid_ir(&program, &document), valid_ir(&program, &document));
    assert_eq!(emit(&first), emit(&second));
}

const EVERYTHING: &str = r#"language: velme/0.1

type Item:
    k: Number
    name: Text

goal Everything(xs: List<Number>, items: List<Item>, s: Text, n: Number?) -> Item:
    plan: "Something of everything."
"#;

fn number(n: &str) -> String {
    format!(r#"{{"kind": "literal", "type": {{"t": "Number"}}, "value": {n}}}"#)
}

fn input(name: &str) -> String {
    format!(r#"{{"kind": "input", "name": "{name}"}}"#)
}

fn local(name: &str) -> String {
    format!(r#"{{"kind": "local", "name": "{name}"}}"#)
}

fn binary(op: &str, left: &str, right: &str) -> String {
    format!(r#"{{"kind": "binary", "op": "{op}", "left": {left}, "right": {right}}}"#)
}

fn unary(op: &str, arg: &str) -> String {
    format!(r#"{{"kind": "unary", "op": "{op}", "arg": {arg}}}"#)
}

fn builtin(name: &str, args: &[&str]) -> String {
    format!(
        r#"{{"kind": "builtin", "name": "{name}", "args": [{}]}}"#,
        args.join(", ")
    )
}

fn collection(kind: &str, list: &str, param: &str, body: &str) -> String {
    format!(r#"{{"kind": "{kind}", "list": {list}, "fn": {{"param": "{param}", "body": {body}}}}}"#)
}

fn field(of: &str, name: &str) -> String {
    format!(r#"{{"kind": "field", "of": {of}, "field": "{name}"}}"#)
}

fn conditional(cond: &str, then: &str, otherwise: &str) -> String {
    format!(r#"{{"kind": "if", "cond": {cond}, "then": {then}, "else": {otherwise}}}"#)
}

fn unwrap_or(of: &str, default: &str) -> String {
    format!(r#"{{"kind": "unwrap_or", "of": {of}, "default": {default}}}"#)
}

fn numbers(items: &[&str]) -> String {
    format!(
        r#"{{"kind": "list", "of": {{"t": "Number"}}, "items": [{}]}}"#,
        items.join(", ")
    )
}

fn item(k: &str, name: &str) -> String {
    format!(r#"{{"kind": "record", "type": "Item", "fields": {{"k": {k}, "name": {name}}}}}"#)
}

/// The IR document of the goal of [`EVERYTHING`] with `body`.
fn document(body: &str) -> String {
    let number = r#"{"t": "Number"}"#;
    let item = r#"{"t": "Record", "name": "Item"}"#;
    format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Everything",
            "types": {{"Item": {{"fields": [["k", {number}], ["name", {{"t": "Text"}}]]}}}},
            "inputs": [["xs", {{"t": "List", "of": {number}}}], ["items", {{"t": "List", "of": {item}}}],
                ["s", {{"t": "Text"}}], ["n", {{"t": "Optional", "of": {number}}}]],
            "output": {item}, "body": {body}}}"#
    )
}

/// The IR of a goal of [`EVERYTHING`] whose body has every node kind a body may hold, every operator and every
/// value built-in.
fn everything() -> String {
    let zero = number("0");
    let bindings = [
        (
            "a",
            collection("map", &input("xs"), "v", &binary("add", &local("v"), &number("1"))),
        ),
        (
            "b",
            collection(
                "filter",
                &local("a"),
                "w",
                &binary(
                    "or",
                    &binary(
                        "and",
                        &binary("gt", &local("w"), &number("1")),
                        &unary("not", &binary("eq", &local("w"), &number("2"))),
                    ),
                    &binary("le", &local("w"), &zero),
                ),
            ),
        ),
        (
            "c",
            collection(
                "find",
                &input("items"),
                "i",
                &binary("ge", &field(&local("i"), "k"), &unwrap_or(&input("n"), &zero)),
            ),
        ),
        (
            "d",
            collection("all", &input("xs"), "p", &binary("ne", &local("p"), &zero)),
        ),
        (
            "e",
            collection(
                "any",
                &input("xs"),
                "q",
                &binary("lt", &local("q"), &unary("neg", &local("q"))),
            ),
        ),
        (
            "f",
            format!(
                r#"{{"kind": "reduce", "list": {}, "init": {zero}, "fn": {{"acc": "acc", "param": "r", "body": {}}}}}"#,
                local("b"),
                binary(
                    "sub",
                    &binary("mul", &local("acc"), &local("r")),
                    &binary("div", &local("acc"), &number("1"))
                )
            ),
        ),
        (
            "g",
            format!(
                r#"{{"kind": "sort_by", "list": {}, "key": {{"param": "t", "body": {}}}, "descending": true}}"#,
                input("items"),
                field(&local("t"), "k")
            ),
        ),
        (
            "h",
            conditional(
                &binary(
                    "and",
                    &unary("is_empty", &local("c")),
                    &binary("and", &local("d"), &local("e")),
                ),
                &numbers(&[&number("1"), &builtin("length", &[&local("g")])]),
                &builtin("range", &[&number("3")]),
            ),
        ),
        (
            "text",
            builtin(
                "concat",
                &[&builtin("to_text", &[&builtin("sum", &[&local("h")])]), &input("s")],
            ),
        ),
        (
            "j",
            binary(
                "add",
                &binary(
                    "add",
                    &builtin("length", &[&input("xs")]),
                    &builtin("length", &[&input("s")]),
                ),
                &binary(
                    "add",
                    &builtin(
                        "abs",
                        &[&builtin(
                            "floor",
                            &[&builtin(
                                "ceil",
                                &[&builtin(
                                    "round",
                                    &[&builtin("clamp", &[&local("f"), &zero, &number("9")])],
                                )],
                            )],
                        )],
                    ),
                    &builtin("random", &[&number("1"), &number("2")]),
                ),
            ),
        ),
        (
            "k",
            conditional(
                &binary(
                    "and",
                    &builtin("contains", &[&input("xs"), &number("3")]),
                    &builtin("is_empty", &[&input("s")]),
                ),
                &builtin("maximum", &[&input("xs")]),
                &builtin("minimum", &[&input("xs")]),
            ),
        ),
    ];
    let bind: Vec<String> = bindings
        .iter()
        .map(|(name, value)| format!(r#"["{name}", {value}]"#))
        .collect();
    let result = item(&unwrap_or(&local("k"), &local("j")), &local("text"));
    document(&format!(
        r#"{{"kind": "let", "bind": [{}], "body": {result}}}"#,
        bind.join(", ")
    ))
}

/// The golden set has no `sort_by` of records nor most built-ins, so one goal holds every node kind, operator and
/// value built-in there is, and the test says so if the catalog grows.
#[test]
fn ac_sbx_06_every_node_kind_and_builtin_validates() {
    let ir = valid_ir(&program(EVERYTHING), &everything());
    let nodes = ir.goal().body.preorder();
    let kinds: HashSet<_> = nodes.iter().map(|node| std::mem::discriminant(*node)).collect();
    // Every kind of `compiler/21` §3 but `call`, which no body may hold (R-IR-02).
    assert_eq!(kinds.len(), 19);
    let operators: BTreeSet<String> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::BinaryOp { op, .. } => to_canonical_string(op).ok(),
            Node::UnaryOp { op, .. } => to_canonical_string(op).ok(),
            _ => None,
        })
        .collect();
    assert_eq!(operators.len(), 15, "{operators:?}");
    let called: BTreeSet<&str> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Builtin { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let catalog: BTreeSet<&str> = CATALOG
        .iter()
        .filter(|b| b.function.is_some())
        .map(|b| b.name)
        .collect();
    assert_eq!(called, catalog);
    assert_module("Everything", &ir);
}

/// A body at the validator's limits still fits one function: locals are reused, so neither a deep expression nor a
/// long list runs into a validator's limit on them.
#[test]
fn ac_sbx_06_a_deep_body_and_a_long_list_validate() {
    let program = program(EVERYTHING);
    let item = |k: &str| item(k, &input("s"));
    let mut deep = number("1");
    for _ in 0..velme_ir::limits::MAX_DEPTH - 3 {
        deep = binary("add", &number("1"), &deep);
    }
    assert_module("deep", &valid_ir(&program, &document(&item(&deep))));
    let items: Vec<String> = (0..velme_ir::limits::MAX_LIST_ITEMS)
        .map(|i| item(&number(&i.to_string())))
        .collect();
    let long = unwrap_or(
        &collection(
            "find",
            &format!(
                r#"{{"kind": "list", "of": {{"t": "Record", "name": "Item"}}, "items": [{}]}}"#,
                items.join(", ")
            ),
            "i",
            &binary("gt", &field(&local("i"), "k"), &number("5")),
        ),
        &item(&number("0")),
    );
    assert_module("long", &valid_ir(&program, &document(&long)));
}

/// A module of one function `() -> ()` whose body is `body`, by hand: what the emitter never makes.
fn hand_made(locals: &[ValType], body: &[Instruction<'_>]) -> Vec<u8> {
    hand_made_with(&[], false, locals, body)
}

/// A linear memory of one page.
fn memory(memory64: bool) -> MemoryType {
    MemoryType {
        minimum: 1,
        maximum: None,
        memory64,
        shared: false,
        page_size_log2: None,
    }
}

/// [`hand_made`], with `memories` and, if `tag`, one exception tag of no values.
fn hand_made_with(memories: &[MemoryType], tag: bool, locals: &[ValType], body: &[Instruction<'_>]) -> Vec<u8> {
    let mut module = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    if !memories.is_empty() {
        let mut section = MemorySection::new();
        for memory in memories {
            section.memory(*memory);
        }
        module.section(&section);
    }
    if tag {
        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: 0,
        });
        module.section(&tags);
    }
    let mut function = Function::new_with_locals_types(locals.iter().copied());
    for instruction in body {
        function.instruction(instruction);
    }
    function.instruction(&Instruction::End);
    let mut code = CodeSection::new();
    code.function(&function);
    module.section(&code);
    module.finish()
}

// The float half of AC-SBX-05; its other half, a division by zero trapping with `VL0602`, needs a module to run.
#[test]
fn ac_sbx_05_a_float_instruction_is_rejected_by_validation() {
    use Instruction as I;
    // The same shape without a float is a module, so it is the float that is refused.
    assert_eq!(validate(&hand_made(&[], &[I::I64Const(1), I::Drop])), Ok(()));
    let floats: [&[I<'_>]; 4] = [
        &[I::F32Const(1.0.into()), I::Drop],
        &[I::F64Const(1.0.into()), I::Drop],
        &[I::I64Const(1), I::F64ConvertI64S, I::Drop],
        &[I::I32Const(0), I::F32Load(crate::code::mem(0, 2)), I::Drop],
    ];
    for body in floats {
        let error = validate(&hand_made(&[], body)).expect_err("a float operation");
        assert!(error.contains("floating-point"), "{error}");
    }
    // A float type with no operation on it is refused too.
    for float in [ValType::F32, ValType::F64] {
        let error = validate(&hand_made(&[float], &[])).expect_err("a float local");
        assert!(error.contains("floating-point"), "{error}");
    }
}

/// R-SBX-07 is exactly its feature set: what it leaves out is refused, not only floats. No module here has a float,
/// and each is a module once its one feature is on, so it is that feature that is refused.
#[test]
fn ac_sbx_06_features_outside_the_set_are_rejected() {
    use Instruction as I;
    let atomic = [
        I::I32Const(0),
        I::I32Const(0),
        I::MemoryAtomicNotify(crate::code::mem(0, 2)),
        I::Drop,
    ];
    let outside: [(&str, WasmFeatures, Vec<u8>); 10] = [
        (
            "sign extension",
            WasmFeatures::SIGN_EXTENSION,
            hand_made(&[], &[I::I32Const(0), I::I32Extend8S, I::Drop]),
        ),
        (
            "tail calls",
            WasmFeatures::TAIL_CALL,
            hand_made(&[], &[I::ReturnCall(0)]),
        ),
        (
            "threads",
            WasmFeatures::THREADS,
            hand_made_with(&[memory(false)], false, &[], &atomic),
        ),
        (
            "a second memory",
            WasmFeatures::MULTI_MEMORY,
            hand_made_with(&[memory(false), memory(false)], false, &[], &[]),
        ),
        (
            "memory64",
            WasmFeatures::MEMORY64,
            hand_made_with(&[memory(true)], false, &[], &[]),
        ),
        (
            "a reference-type instruction",
            WasmFeatures::REFERENCE_TYPES,
            hand_made(&[], &[I::RefNull(HeapType::FUNC), I::Drop]),
        ),
        (
            "a reference-type local",
            WasmFeatures::REFERENCE_TYPES,
            hand_made(&[ValType::FUNCREF], &[]),
        ),
        (
            "exceptions",
            WasmFeatures::EXCEPTIONS,
            hand_made_with(&[], true, &[], &[I::Throw(0)]),
        ),
        (
            "legacy exceptions",
            WasmFeatures::EXCEPTIONS.union(WasmFeatures::LEGACY_EXCEPTIONS),
            hand_made(&[], &[I::Try(wasm_encoder::BlockType::Empty), I::End]),
        ),
        ("SIMD", WasmFeatures::SIMD, hand_made(&[ValType::V128], &[])),
    ];
    for (name, feature, module) in &outside {
        let with = Validator::new_with_features(FEATURES.union(*feature)).validate_all(module);
        assert!(with.is_ok(), "{name}: {:?}", with.err());
        assert!(validate(module).is_err(), "{name}");
    }
}

#[test]
fn a_composite_goal_is_not_a_leaf() {
    // Its calls stay with the host's scheduler (R-SBX-17), which runs its tail on the interpreter and never asks
    // for a module of it: not one of the declines of R-SBX-02.
    let program = program(&read(&repo("examples/beginner/double_then_add_one.velme")));
    let wired = wired_goal(&program, goal_id(&program, "Main")).expect("a wired goal");
    let ir = valid_ir(&program, &to_canonical_string(&wired).expect("IR"));
    assert_eq!(emit(&ir), Err(EmitError::NotLeaf));
}

/// A program whose record `Wide` holds `depth` levels of records of 100 fields, 100 numbers at the bottom, and a
/// goal `Count` taking `input`; with the IR of a body that gives a number.
fn wide(depth: usize, input: &str) -> (String, String) {
    let mut source = "language: velme/0.1\n".to_owned();
    let mut types = Vec::new();
    for level in 0..depth {
        let name = if level + 1 == depth {
            "Wide".to_owned()
        } else {
            format!("Level{level}")
        };
        let (field_source, field_ir) = match level.checked_sub(1) {
            Some(below) => (
                format!("Level{below}"),
                format!(r#"{{"t": "Record", "name": "Level{below}"}}"#),
            ),
            None => ("Number".to_owned(), r#"{"t": "Number"}"#.to_owned()),
        };
        source.push_str(&format!("\ntype {name}:\n"));
        let mut listed = Vec::new();
        for i in 0..100 {
            source.push_str(&format!("    f{i}: {field_source}\n"));
            listed.push(format!(r#"["f{i}", {field_ir}]"#));
        }
        types.push(format!(r#""{name}": {{"fields": [{}]}}"#, listed.join(", ")));
    }
    let (param, ty) = match input {
        "Wide" => ("Wide", r#"{"t": "Record", "name": "Wide"}"#),
        _ => ("List<Wide>", r#"{"t": "List", "of": {"t": "Record", "name": "Wide"}}"#),
    };
    source.push_str(&format!(
        "\ngoal Count(book: {param}) -> Number:\n    plan: \"How many numbers.\"\n"
    ));
    let document = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Count",
            "types": {{{}}}, "inputs": [["book", {ty}]], "output": {{"t": "Number"}}, "body": {}}}"#,
        types.join(", "),
        number("20000")
    );
    (source, document)
}

#[test]
fn a_record_is_declined_only_when_no_address_reaches_its_slot() {
    // 100 numbers, a hundred of those, and a hundred of those: a slot of 16 MB, far wider than a scratch frame.
    // No frame holds a record, so it is a module.
    let (source, document) = wide(3, "Wide");
    assert_module("wide", &valid_ir(&program(&source), &document));
    // Two levels more is 160 GB, past any address of a 32-bit memory.
    let (source, document) = wide(5, "Wide");
    let too_wide = Err(EmitError::Declined("a record type too wide for one memory"));
    assert_eq!(emit(&valid_ir(&program(&source), &document)), too_wide);
    // A goal that never lays the record out is a module still: a list is a pointer, and the body doesn't read it.
    let (source, document) = wide(5, "List<Wide>");
    assert_module("unused", &valid_ir(&program(&source), &document));
}

/// The IR document of a goal `G(xs: List<Number>)` giving `output`, and its source.
fn nothing_goal(output: &str, output_ir: &str, body: &str) -> ValidIr {
    let source = format!("language: velme/0.1\n\ngoal G(xs: List<Number>) -> {output}:\n    plan: \"Nothing much.\"\n");
    let document = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "G", "types": {{}},
            "inputs": [["xs", {{"t": "List", "of": {{"t": "Number"}}}}]], "output": {output_ir}, "body": {body}}}"#
    );
    valid_ir(&program(&source), &document)
}

#[test]
fn a_list_of_nothing_is_a_module_however_it_is_made() {
    // Its items take no slot and no bytes on either backend (`runtime/30` §7.1), so a `map` to `nothing` and a
    // `list` node of `Nothing` are the same list.
    let nothing = r#"{"kind": "literal", "type": {"t": "Nothing"}, "value": null}"#;
    let list = r#"{"t": "List", "of": {"t": "Nothing"}}"#;
    let mapped = collection("map", &input("xs"), "x", nothing);
    assert_module("map", &nothing_goal("List<Nothing>", list, &mapped));
    let node = format!(r#"{{"kind": "list", "of": {{"t": "Nothing"}}, "items": [{nothing}, {nothing}]}}"#);
    assert_module("list", &nothing_goal("List<Nothing>", list, &node));
    let literal = format!(r#"{{"kind": "literal", "type": {list}, "value": [null, null]}}"#);
    assert_module("literal", &nothing_goal("List<Nothing>", list, &literal));
    // A `Nothing?` has no slot: `find` over such a list is still declined.
    let found = unary(
        "is_empty",
        &collection(
            "find",
            &mapped,
            "y",
            r#"{"kind": "literal", "type": {"t": "Boolean"}, "value": true}"#,
        ),
    );
    assert_eq!(
        emit(&nothing_goal("Boolean", r#"{"t": "Boolean"}"#, &found)),
        Err(EmitError::Declined("a type that holds Nothing"))
    );
}

/// R-SBX-19 (a): a value takes no more linear memory than it is charged. A `T?` is one pointer, so a list of
/// absent ones fits the 8 bytes each is charged, whatever `T` is, and a present `Number?` its 24.
#[test]
fn r_sbx_19_an_optional_takes_no_more_than_it_is_charged() {
    let ir = valid_ir(&program(EVERYTHING), &everything());
    let types = Types::new(ir.goal()).expect("types");
    let count = usize::try_from(MAX_LIST_SIZE).expect("a count");
    let charged = list_bytes(&vec![Value::Nothing; count], true);
    assert_eq!(charged, 16 + 8 * MAX_LIST_SIZE);
    let inner = [
        Ty::Number,
        Ty::Boolean,
        Ty::Text,
        Ty::List(Box::new(Ty::Number)),
        Ty::Record(0),
    ];
    for of in inner {
        let slot = types.slot(&Ty::Optional(Box::new(of.clone()))).expect("a slot");
        let allocated = u64::from(LIST_PREFIX) + u64::from(slot) * MAX_LIST_SIZE;
        assert!(allocated <= charged, "{of:?}: {allocated} bytes for {charged} charged");
    }
    let slot = |ty: &Ty| u64::from(types.slot(ty).expect("a slot"));
    let present = Value::Number(Number::ZERO);
    let optional = Ty::Optional(Box::new(Ty::Number));
    assert_eq!(
        u64::from(LIST_PREFIX) + slot(&optional) + slot(&Ty::Number),
        list_bytes(std::slice::from_ref(&present), true) - 16 + u64::from(LIST_PREFIX)
    );
    assert_eq!(slot(&optional) + slot(&Ty::Number), 8 + value_bytes(&present));
}

#[test]
fn a_module_says_what_its_data_segment_takes() {
    // The host adds it, with the scratch stack, to what a run's memory may grow to (`runtime/31` §6, D-90).
    let ir = valid_ir(&program(EVERYTHING), &document(&item(&number("1"), &input("s"))));
    assert_eq!(emit(&ir).expect("a module").data_bytes(), 0);
    let text = r#"{"kind": "literal", "type": {"t": "Text"}, "value": "nine bytes"}"#;
    let ir = valid_ir(&program(EVERYTHING), &document(&item(&number("1"), text)));
    assert_eq!(emit(&ir).expect("a module").data_bytes(), 16);
}

#[test]
fn reasons_and_imports_keep_their_numbers() {
    // The host reads a reason by its code and links an import by its index's name (R-SBX-11, `runtime/31` §5).
    for (code, reason) in Reason::ALL.into_iter().enumerate() {
        assert_eq!(usize::try_from(reason.code()).ok(), Some(code));
        assert_eq!(i32::try_from(code).ok().and_then(Reason::from_code), Some(reason));
    }
    assert_eq!(Reason::from_code(5), None);
    assert_eq!(Reason::from_code(-1), None);
    for (index, import) in Import::ALL.into_iter().enumerate() {
        assert_eq!(import.index() as usize, index);
    }
    let names: BTreeSet<&str> = Import::ALL.iter().map(|i| i.name()).collect();
    assert_eq!(names.len(), Import::ALL.len());
}
