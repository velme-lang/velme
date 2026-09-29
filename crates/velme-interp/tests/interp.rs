//! The reference interpreter (`runtime/30` §3): the golden IR corpus run on inputs, and the run-time criteria of
//! `compiler/21` and `language/14` it owns.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use serde_json::Value as Json;
use velme_builtins::limits::MAX_FUEL;
use velme_builtins::{BUILTINS_VERSION, Value};
use velme_diagnostics::{Code, Span};
use velme_interp::{Error, Evaluator, Failure, Output, Probe, run};
use velme_ir::limits::MAX_NODES;
use velme_ir::{IR_VERSION, Node, ValidIr, decode_str, encode_value, from_json_str};
use velme_test_support::{goal_id, program, read, repo, valid_ir};

/// The goals the IR documents below implement.
const GOALS: &str = r#"language: velme/0.1

type Item:
    k: Number
    name: Text

type Player:
    score: Number
    name: Text

goal FirstBig(xs: List<Number>) -> Number?:
    plan: "The first number over 10."

goal Inverses(xs: List<Number>) -> List<Number>:
    plan: "One over each number."

goal SortItems(items: List<Item>) -> List<Item>:
    plan: "The items by k."

goal PairScore(a: Player?, b: Player?) -> Number:
    plan: "Both scores added, or 0 when either player is missing."

goal Compute(x: Number?) -> Number:
    plan: "Something of x."

goal Present(p: Player?) -> Boolean:
    plan: "Whether there is a player."
"#;

const ITEM: &str = r#""Item": {"fields": [["k", {"t": "Number"}], ["name", {"t": "Text"}]]}"#;
const PLAYER: &str = r#""Player": {"fields": [["score", {"t": "Number"}], ["name", {"t": "Text"}]]}"#;

/// The IR document of a [`GOALS`] goal with `body`.
fn document(goal: &str, body: &str) -> String {
    let (types, inputs, output) = match goal {
        "FirstBig" => (
            String::new(),
            r#"[["xs", {"t": "List", "of": {"t": "Number"}}]]"#,
            r#"{"t": "Optional", "of": {"t": "Number"}}"#,
        ),
        "Inverses" => (
            String::new(),
            r#"[["xs", {"t": "List", "of": {"t": "Number"}}]]"#,
            r#"{"t": "List", "of": {"t": "Number"}}"#,
        ),
        "SortItems" => (
            ITEM.to_owned(),
            r#"[["items", {"t": "List", "of": {"t": "Record", "name": "Item"}}]]"#,
            r#"{"t": "List", "of": {"t": "Record", "name": "Item"}}"#,
        ),
        "PairScore" => (
            PLAYER.to_owned(),
            r#"[["a", {"t": "Optional", "of": {"t": "Record", "name": "Player"}}],
                ["b", {"t": "Optional", "of": {"t": "Record", "name": "Player"}}]]"#,
            r#"{"t": "Number"}"#,
        ),
        "Compute" => (
            String::new(),
            r#"[["x", {"t": "Optional", "of": {"t": "Number"}}]]"#,
            r#"{"t": "Number"}"#,
        ),
        "Present" => (
            PLAYER.to_owned(),
            r#"[["p", {"t": "Optional", "of": {"t": "Record", "name": "Player"}}]]"#,
            r#"{"t": "Boolean"}"#,
        ),
        _ => panic!("no goal {goal}"),
    };
    format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "{goal}",
            "types": {{{types}}}, "inputs": {inputs}, "output": {output}, "body": {body}}}"#
    )
}

/// `value` as JSON, which fits the output limit.
fn encode(value: &Value) -> String {
    encode_value(value).expect("fits the output limit")
}

/// Runs `body` as the goal `goal` of [`GOALS`] on the JSON `inputs`, one per parameter.
fn eval(goal: &str, body: &str, inputs: &[&str]) -> Result<Output, Failure> {
    eval_with(goal, body, inputs, MAX_FUEL)
}

fn eval_with(goal: &str, body: &str, inputs: &[&str], max_fuel: u64) -> Result<Output, Failure> {
    let program = program(GOALS);
    let ir = valid_ir(&program, &document(goal, body));
    let params = &program.goals[goal_id(&program, goal).0].params;
    assert_eq!(params.len(), inputs.len());
    let inputs = params
        .iter()
        .zip(inputs)
        .map(|(p, text)| decode_str(text, &p.ty, &program).expect("input decodes"))
        .collect();
    run(&ir, inputs, Vec::new(), max_fuel)
}

/// The output of a run that must succeed, as JSON.
fn value(goal: &str, body: &str, inputs: &[&str]) -> String {
    encode(&eval(goal, body, inputs).unwrap_or_else(|f| panic!("{f:?}")).value)
}

fn failure(goal: &str, body: &str, inputs: &[&str]) -> Failure {
    eval(goal, body, inputs).expect_err("the run fails")
}

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

fn builtin(name: &str, args: &[&str]) -> String {
    format!(
        r#"{{"kind": "builtin", "name": "{name}", "args": [{}]}}"#,
        args.join(", ")
    )
}

// ---- golden corpus ----

/// Every golden IR file runs on the cases of `tests/golden/ir/run/`, so every node kind has an interpreter test
/// (`compiler/21` R-IR-23): each case gives JSON `inputs` by parameter name and, for a composite goal, `bindings`
/// standing in for its children. The snapshot is each case's output and fuel.
#[test]
fn golden_ir_runs() {
    let program = program(&read(&repo("tests/golden/ir/goals.velme")));
    insta::glob!("../../../tests/golden/ir/accept", "*.json", |path| {
        let ir = valid_ir(&program, &read(path));
        let goal = &program.goals[goal_id(&program, &ir.goal().goal).0];
        let name = path.file_name().and_then(|n| n.to_str()).expect("file name");
        let cases: Json = from_json_str(&read(&repo(&format!("tests/golden/ir/run/{name}")))).expect("cases");
        let mut out = String::new();
        for case in cases.as_array().expect("a list of cases") {
            let decode = |json: &Json, ty| decode_str(&json.to_string(), ty, &program).expect("decodes");
            let inputs = goal
                .params
                .iter()
                .map(|p| decode(&case["inputs"][&p.name], &p.ty))
                .collect();
            let bindings = goal
                .bindings
                .iter()
                .map(|b| decode(&case["bindings"][&b.name], &b.ty))
                .collect();
            let Output { value, fuel } = run(&ir, inputs, bindings, MAX_FUEL).expect("runs");
            out.push_str(&format!("{} -> {} (fuel {fuel})\n", case["inputs"], encode(&value)));
        }
        insta::assert_snapshot!(out);
    });
}

#[test]
fn ac_ir_08_player_summary_runs_with_stub_children() {
    let program = program(&read(&repo("tests/golden/ir/goals.velme")));
    let ir = valid_ir(&program, &read(&repo("tests/golden/ir/accept/player_summary.json")));
    let player = Value::record(
        "Player",
        vec![
            ("name".to_owned(), Value::text("Lina")),
            ("jump_height".to_owned(), Value::Number(3i64.into())),
            ("score".to_owned(), Value::Number(820i64.into())),
        ],
    );
    // The children's results: what CalculateScore, CalculateRank and FindBadge would return.
    let bindings = vec![
        Value::Number(820i64.into()),
        Value::Number(2i64.into()),
        Value::text("Jumper"),
    ];
    let output = run(&ir, vec![player], bindings, MAX_FUEL).expect("runs");
    assert_eq!(encode(&output.value), r#"{"score":820,"rank":2,"badge":"Jumper"}"#);
    // The record node and its three locals.
    assert_eq!(output.fuel, 4);
}

// ---- collections ----

fn lambda(param: &str, body: &str) -> String {
    format!(r#"{{"param": "{param}", "body": {body}}}"#)
}

#[test]
fn ac_blt_07_find_returns_the_first_match_or_nothing() {
    let body = format!(
        r#"{{"kind": "find", "list": {}, "fn": {}}}"#,
        input("xs"),
        lambda("x", &binary("gt", &local("x"), &number("10")))
    );
    assert_eq!(value("FirstBig", &body, &["[3, 12, 40, 11]"]), "12");
    assert_eq!(value("FirstBig", &body, &["[3, 4]"]), "null");
    assert_eq!(value("FirstBig", &body, &["[]"]), "null");
}

/// The elements a collection node visits.
#[derive(Default)]
struct Visits(Vec<(usize, String)>);

impl Probe for Visits {
    fn value(&mut self, _: &Node, _: &Value) {}

    fn element(&mut self, _: &Node, index: usize, element: &Value) {
        self.0.push((index, encode(element)));
    }

    fn failed(&mut self, _: &Node, _: &Failure) {}
}

#[test]
fn ac_blt_15_failing_lambda_names_its_element_and_stops() {
    let body = format!(
        r#"{{"kind": "map", "list": {}, "fn": {}}}"#,
        input("xs"),
        lambda("p", &binary("div", &number("1"), &local("p")))
    );
    let f = failure("Inverses", &body, &["[2, 0, 3]"]);
    assert_eq!(f.code(), Code::ArithmeticError);
    assert_eq!(f.elements, [1]);
    let diag = f.diagnostic("Inverses", Span::new(0, 4));
    assert_eq!(diag.message, "`Inverses` tried to divide 1 by 0, which has no answer.");
    assert_eq!(diag.notes, ["this happened at item 1 of a list (counting from 0)"]);

    // The third element is never visited.
    let program = program(GOALS);
    let ir = valid_ir(&program, &document("Inverses", &body));
    let params = &program.goals[goal_id(&program, "Inverses").0].params;
    let xs = decode_str("[2, 0, 3]", &params[0].ty, &program).expect("decodes");
    let mut evaluator = Evaluator::new(MAX_FUEL);
    evaluator.bind_input("xs", xs);
    let mut visits = Visits::default();
    assert!(evaluator.eval_probed(ir.body(), &mut visits).is_err());
    assert_eq!(visits.0, [(0, "2".to_owned()), (1, "0".to_owned())]);
}

#[test]
fn ac_blt_13_sort_by_keeps_equal_keys_in_input_order() {
    let items = r#"[{"k": 2, "name": "a"}, {"k": 1, "name": "b"}, {"k": 1, "name": "c"}]"#;
    let sort = |descending: bool| {
        let body = format!(
            r#"{{"kind": "sort_by", "list": {}, "key": {}, "descending": {descending}}}"#,
            input("items"),
            lambda(
                "i",
                r#"{"kind": "field", "of": {"kind": "local", "name": "i"}, "field": "k"}"#
            )
        );
        let names: Json = serde_json::from_str(&value("SortItems", &body, &[items])).expect("JSON");
        names
            .as_array()
            .expect("a list")
            .iter()
            .map(|item| item["name"].as_str().expect("a name").to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(sort(false), ["b", "c", "a"]);
    assert_eq!(sort(true), ["a", "b", "c"]);
}

#[test]
fn reduce_folds_left_from_its_initial_value() {
    let body = format!(
        r#"{{"kind": "reduce", "list": {}, "init": {}, "fn": {{"acc": "a", "param": "x", "body": {}}}}}"#,
        builtin("range", &[&number("4")]),
        number("100"),
        binary("sub", &local("a"), &local("x"))
    );
    // ((((100 - 0) - 1) - 2) - 3)
    assert_eq!(value("Compute", &body, &["null"]), "94");
}

// ---- narrowing, conditions, short-circuits ----

/// `if a is empty or b is empty then 0 else a.score + b.score`, narrowed explicitly as IR requires (R-IR-05).
#[test]
fn ac_typ_20_if_else_over_narrowed_optionals() {
    let empty = |name: &str| format!(r#"{{"kind": "unary", "op": "is_empty", "arg": {}}}"#, input(name));
    let nobody = r#"{"kind": "literal", "type": {"t": "Record", "name": "Player"}, "value": {"name": "", "score": 0}}"#;
    let score = |name: &str| {
        format!(
            r#"{{"kind": "field", "field": "score", "of": {{"kind": "unwrap_or", "of": {}, "default": {nobody}}}}}"#,
            input(name)
        )
    };
    let body = format!(
        r#"{{"kind": "if", "cond": {}, "then": {}, "else": {}}}"#,
        binary("or", &empty("a"), &empty("b")),
        number("0"),
        binary("add", &score("a"), &score("b"))
    );
    let lina = r#"{"name": "Lina", "score": 3}"#;
    let bo = r#"{"name": "Bo", "score": 4.5}"#;
    assert_eq!(value("PairScore", &body, &[lina, bo]), "7.5");
    assert_eq!(value("PairScore", &body, &[lina, "null"]), "0");
    assert_eq!(value("PairScore", &body, &["null", bo]), "0");
}

#[test]
fn unwrap_or_evaluates_its_default_only_for_nothing() {
    let body = format!(
        r#"{{"kind": "unwrap_or", "of": {}, "default": {}}}"#,
        input("x"),
        binary("div", &number("1"), &number("0"))
    );
    assert_eq!(value("Compute", &body, &["5"]), "5");
    assert_eq!(failure("Compute", &body, &["null"]).code(), Code::ArithmeticError);
}

#[test]
fn and_or_skip_their_right_operand_once_decided() {
    let error = binary("gt", &binary("div", &number("1"), &number("0")), &number("0"));
    let boolean = |b: bool| format!(r#"{{"kind": "literal", "type": {{"t": "Boolean"}}, "value": {b}}}"#);
    let pick = |cond: &str| {
        format!(
            r#"{{"kind": "if", "cond": {cond}, "then": {}, "else": {}}}"#,
            number("1"),
            number("2")
        )
    };
    assert_eq!(
        value("Compute", &pick(&binary("and", &boolean(false), &error)), &["null"]),
        "2"
    );
    assert_eq!(
        value("Compute", &pick(&binary("or", &boolean(true), &error)), &["null"]),
        "1"
    );
    let f = failure("Compute", &pick(&binary("and", &boolean(true), &error)), &["null"]);
    assert_eq!(f.code(), Code::ArithmeticError);
}

// ---- failures and fuel ----

#[test]
fn ac_run_08_division_by_zero_fails_with_no_value() {
    // AC-TYP-06 on the interpreter.
    let f = failure("Compute", &binary("div", &number("1"), &number("0")), &["null"]);
    assert_eq!(f.code(), Code::ArithmeticError);
    assert!(f.elements.is_empty());
}

#[test]
fn ac_blt_12_builtin_fuel_is_its_catalog_cost() {
    let sum = builtin("sum", &[&builtin("range", &[&number("1000")])]);
    let output = eval("Compute", &sum, &["null"]).expect("runs");
    assert_eq!(encode(&output.value), "499500");
    // sum 1 + 1000, range 1 + 1000, and the literal.
    assert_eq!(output.fuel, 2003);

    let contains = format!(
        r#"{{"kind": "if", "cond": {}, "then": {}, "else": {}}}"#,
        builtin("contains", &[&builtin("range", &[&number("100")]), &number("4")]),
        number("1"),
        number("0")
    );
    // if 1, contains 1 + 5, range 1 + 100, its literal, the needle, then the `then` literal.
    assert_eq!(eval("Compute", &contains, &["null"]).expect("runs").fuel, 111);
}

#[test]
fn collections_charge_one_per_element_visited() {
    let map = format!(
        r#"{{"kind": "map", "list": {}, "fn": {}}}"#,
        input("xs"),
        lambda("x", &local("x"))
    );
    // map 1 + 3 visits, the input, and three lambda bodies.
    assert_eq!(eval("Inverses", &map, &["[1, 2, 3]"]).expect("runs").fuel, 8);

    let sort = format!(
        r#"{{"kind": "sort_by", "list": {}, "key": {}, "descending": false}}"#,
        input("items"),
        lambda(
            "i",
            r#"{"kind": "field", "of": {"kind": "local", "name": "i"}, "field": "k"}"#
        )
    );
    let items = r#"[{"k": 3, "name": "a"}, {"k": 1, "name": "b"}, {"k": 2, "name": "c"}]"#;
    // sort_by 1 + 3·⌈log2 4⌉ + 3 keys, the input, and each key's field and local.
    assert_eq!(
        eval("SortItems", &sort, &[items]).expect("runs").fuel,
        1 + 6 + 3 + 1 + 6
    );
}

#[test]
fn equality_of_composite_values_charges_the_pairs_visited() {
    let list = |items: &[&str]| {
        let items: Vec<String> = items.iter().map(|n| number(n)).collect();
        format!(
            r#"{{"kind": "list", "of": {{"t": "Number"}}, "items": [{}]}}"#,
            items.join(", ")
        )
    };
    let outcome = |left: &[&str], right: &[&str]| {
        let cond = binary("eq", &list(left), &list(right));
        let body = format!(
            r#"{{"kind": "if", "cond": {cond}, "then": {}, "else": {}}}"#,
            number("1"),
            number("0")
        );
        let output = eval("Compute", &body, &["null"]).expect("runs");
        (encode(&output.value), output.fuel)
    };
    // `if`, `eq`, both lists and their literals, and the branch's literal; then the pairs visited, the lists' own
    // included (D-83).
    assert_eq!(
        outcome(&["1", "2", "3"], &["1", "2", "3"]),
        ("1".to_owned(), 11 + 1 + 3)
    );
    assert_eq!(
        outcome(&["1", "3", "2"], &["1", "2", "3"]),
        ("0".to_owned(), 11 + 1 + 2)
    );
    // Lists of different lengths differ once their own pair is visited, before any item.
    assert_eq!(outcome(&["1", "2"], &["1", "2", "3"]), ("0".to_owned(), 10 + 1));
}

#[test]
fn running_out_of_fuel_is_a_budget_failure() {
    let sum = builtin("sum", &[&builtin("range", &[&number("1000")])]);
    let f = eval_with("Compute", &sum, &["null"], 2002).expect_err("out of fuel");
    assert_eq!(f.error, Error::OutOfFuel { max_fuel: 2002 });
    assert_eq!(f.code(), Code::BudgetExceeded);
    let diag = f.diagnostic("Compute", Span::new(0, 4));
    assert_eq!(diag.message, "`Compute` took too many steps and was stopped.");
    assert!(eval_with("Compute", &sum, &["null"], 2003).is_ok());
}

#[test]
fn range_above_the_list_limit_is_a_size_failure() {
    let f = failure(
        "Compute",
        &builtin("sum", &[&builtin("range", &[&number("10001")])]),
        &["null"],
    );
    assert_eq!(f.code(), Code::SizeLimitExceeded);
}

#[test]
fn record_fields_evaluate_in_declaration_order() {
    // The node's fields are keyed, so `name` comes first, but `Player` declares `score` first: its failure is the one
    // reported.
    let body = format!(
        r#"{{"kind": "field", "field": "score", "of": {{"kind": "record", "type": "Player", "fields": {{
            "name": {{"kind": "builtin", "name": "to_text", "args": [{}]}},
            "score": {{"kind": "builtin", "name": "round", "args": [{}]}}}}}}}}"#,
        binary("div", &number("2"), &number("0")),
        binary("div", &number("1"), &number("0"))
    );
    let program = program(GOALS);
    let ir: ValidIr = valid_ir(&program, &document("PairScore", &body));
    let f = run(&ir, vec![Value::Nothing, Value::Nothing], Vec::new(), MAX_FUEL).expect_err("fails");
    let diag = f.diagnostic("PairScore", Span::new(0, 4));
    assert_eq!(diag.message, "`PairScore` tried to divide 1 by 0, which has no answer.");
}

#[test]
fn equality_of_a_record_with_nothing_charges_one_pair() {
    let body = binary(
        "ne",
        &input("p"),
        r#"{"kind": "literal", "type": {"t": "Nothing"}, "value": null}"#,
    );
    let present = eval("Present", &body, &[r#"{"score": 1, "name": "Lina"}"#]).expect("runs");
    // `ne`, the input and the literal, then the one pair visited.
    assert_eq!((encode(&present.value), present.fuel), ("true".to_owned(), 4));
    // `nothing` against `nothing` compares no composite value.
    let absent = eval("Present", &body, &["null"]).expect("runs");
    assert_eq!((encode(&absent.value), absent.fuel), ("false".to_owned(), 3));
}

/// A builtin's catalog cost is charged after it computes, so its own failure wins over running out of fuel on the
/// same step (R-RUN-04).
#[test]
fn a_builtin_failure_wins_over_running_out_of_fuel() {
    let body = builtin("sum", &[&builtin("range", &[&number("20000")])]);
    let f = eval_with("Compute", &body, &["null"], 10).expect_err("fails");
    assert_eq!(f.code(), Code::SizeLimitExceeded);
}

/// `Present`'s body `let a0 = <first>, a1 = map(range(2), p1 -> a0), …, a60 = … in <test>`: `a60` holds 2^60 copies
/// of `a0`, which share their parts, so building it is cheap and only walking it isn't.
fn shared_from(first: &str, test: &str) -> String {
    let mut bind = vec![format!(r#"["a0", {first}]"#)];
    for i in 1..=60 {
        let map = format!(
            r#"{{"kind": "map", "list": {}, "fn": {}}}"#,
            builtin("range", &[&number("2")]),
            lambda(&format!("p{i}"), &local(&format!("a{}", i - 1)))
        );
        bind.push(format!(r#"["a{i}", {map}]"#));
    }
    format!(r#"{{"kind": "let", "bind": [{}], "body": {test}}}"#, bind.join(", "))
}

/// [`shared_from`] with `a0 = [1]`.
fn shared(test: &str) -> String {
    let first = format!(
        r#"{{"kind": "list", "of": {{"t": "Number"}}, "items": [{}]}}"#,
        number("1")
    );
    shared_from(&first, test)
}

/// Equality is charged as it compares (D-83): comparing values that share their parts stops where the fuel runs out,
/// with `VL0601`, rather than first walking 2^60 leaves.
#[test]
fn ac_run_05_equality_over_shared_values_stops_at_the_fuel_limit() {
    let eq = shared(&binary("eq", &local("a60"), &local("a60")));
    let f = failure("Present", &eq, &["null"]);
    assert_eq!(f.error, Error::OutOfFuel { max_fuel: MAX_FUEL });
    // With `a0 = []` there is no leaf at all, but every pair of lists visited is paid for.
    let empty = shared_from(
        r#"{"kind": "list", "of": {"t": "Number"}, "items": []}"#,
        &binary("eq", &local("a60"), &local("a60")),
    );
    assert_eq!(
        failure("Present", &empty, &["null"]).error,
        Error::OutOfFuel { max_fuel: MAX_FUEL }
    );
    // `contains([ak], ak)`, where `ak` is a `List` nested k + 1 deep.
    let contains = |k: usize| {
        let of = (0..k).fold(r#"{"t": "List", "of": {"t": "Number"}}"#.to_owned(), |of, _| {
            format!(r#"{{"t": "List", "of": {of}}}"#)
        });
        let item = local(&format!("a{k}"));
        let list = format!(r#"{{"kind": "list", "of": {of}, "items": [{item}]}}"#);
        shared(&builtin("contains", &[&list, &item]))
    };
    assert_eq!(value("Present", &contains(1), &["null"]), "true");
    assert_eq!(
        failure("Present", &contains(60), &["null"]).code(),
        Code::BudgetExceeded
    );
}

/// `runtime/30` R-RUN-25: the deepest value a goal body can build — a literal nested nearly as deep as the JSON limit
/// allows, then a `map` per level with the rest of `MAX_NODES` — is validated, built, compared and dropped within the
/// CLI's stack.
#[test]
fn the_deepest_value_ir_can_build_fits_the_stack() {
    const LITERAL_DEPTH: usize = 480;
    // `let`, `a0`, three nodes per level, and `an == an`.
    let levels = (MAX_NODES - 5) / 3;
    let body = |levels: usize| {
        let ty = (0..LITERAL_DEPTH).fold(r#"{"t": "Number"}"#.to_owned(), |of, _| {
            format!(r#"{{"t": "List", "of": {of}}}"#)
        });
        let value = format!("{}1{}", "[".repeat(LITERAL_DEPTH), "]".repeat(LITERAL_DEPTH));
        let mut bind = vec![format!(
            r#"["a0", {{"kind": "literal", "type": {ty}, "value": {value}}}]"#
        )];
        for i in 1..=levels {
            let map = format!(
                r#"{{"kind": "map", "list": {}, "fn": {}}}"#,
                local("a0"),
                lambda(&format!("p{i}"), &local(&format!("a{}", i - 1)))
            );
            bind.push(format!(r#"["a{i}", {map}]"#));
        }
        let last = local(&format!("a{levels}"));
        format!(
            r#"{{"kind": "let", "bind": [{}], "body": {}}}"#,
            bind.join(", "),
            binary("eq", &last, &last)
        )
    };
    let deepest = body(levels);
    let outcome = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let program = program(GOALS);
            let doc = document("Present", &deepest);
            let one_more = document("Present", &body(levels + 1));
            let calls = velme_ir::calls(&program, goal_id(&program, "Present")).expect("calls");
            let request = velme_ir::Request {
                program: &program,
                goal: goal_id(&program, "Present"),
                calls: &calls,
                origin: velme_ir::Origin::Complete,
            };
            // One level more passes `MAX_NODES` (stage 7).
            let rejected = velme_ir::validate(&one_more, &request)
                .is_err_and(|diags| diags.iter().any(|d| d.message.contains("expressions")));
            let ir = valid_ir(&program, &doc);
            let output = run(&ir, vec![Value::Nothing], Vec::new(), MAX_FUEL).expect("runs");
            (rejected, output.value)
        })
        .expect("thread")
        .join()
        .expect("fits the stack");
    assert_eq!(outcome, (true, Value::Boolean(true)));
    assert_eq!(levels, 3331);
}

/// Text equality and `length` of a text cost ⌈bytes/64⌉ more, and nothing more for empty text (D-83).
#[test]
fn ac_blt_12_text_equality_and_length_charge_their_bytes() {
    let text = |t: &str| format!(r#"{{"kind": "literal", "type": {{"t": "Text"}}, "value": "{t}"}}"#);
    let long = "a".repeat(65);
    let fuel = |body: &str| eval("Present", body, &["null"]).expect("runs").fuel;
    // `eq` and its two literals, then ⌈65/64⌉ = 2.
    assert_eq!(fuel(&binary("eq", &text(&long), &text(&long))), 3 + 2);
    assert_eq!(fuel(&binary("eq", &text(""), &text(""))), 3);
    // Texts of different byte lengths differ before any byte is compared.
    assert_eq!(fuel(&binary("ne", &text(&long), &text("a"))), 3);
    let length = binary("gt", &builtin("length", &[&text(&long)]), &number("0"));
    // `gt`, `length` 1 + 2, its literal, and the `0`.
    assert_eq!(fuel(&length), 1 + 3 + 1 + 1);
}

/// `reduce` evaluates its list, then its `init` (R-RUN-02): with both failing, the list's failure is reported.
#[test]
fn reduce_evaluates_its_list_before_init() {
    let body = format!(
        r#"{{"kind": "reduce", "list": {}, "init": {}, "fn": {{"acc": "acc", "param": "x", "body": {}}}}}"#,
        builtin("range", &[&number("-1")]),
        binary("div", &number("1"), &number("0")),
        local("acc")
    );
    let diag = failure("Compute", &body, &["null"]).diagnostic("Compute", Span::new(0, 4));
    assert!(diag.message.contains("range"), "{}", diag.message);
}
