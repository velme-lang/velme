//! Domains in IR 0.2 (`compiler/21` R-IR-24, R-IR-25): the compiler's `domains` table, Domain references in
//! signatures, base-valued bodies and the expected-type pass (AC-IR-15..21).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use serde_json::{Value, json};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Origin, Request, ValidIr, from_json_str, goal_tables, ir_type, to_canonical_string, validate};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};

const SOURCE: &str = "language: velme/0.2

type Risk = Text one of \"LOW\", \"HIGH\"
type Age = Number where value >= 18 and value <= 120 and value is whole
type Twin = Number where value >= 18 and value <= 120 and value is whole
type Score = Number where 0.10 <= value and value != 13
type Label = Text where value.length >= 1 and value.length <= 40
type Offer where value.discount <= value.price:
    price: Number
    discount: Number
type Person:
    age: Age
    risk: Risk?

goal Classify(risk: Risk) -> Boolean:
    plan: \"p\"

goal Older(person: Person) -> Person:
    plan: \"p\"

goal Ages(xs: List<Number>, c: Boolean, score: Score) -> List<Age>:
    plan: \"p\"

goal MaybeAges(xs: List<Number?>) -> List<Age?>?:
    plan: \"p\"

goal Pick(age: Age, score: Score, twin: Twin, c: Boolean) -> Age:
    plan: \"p\"

goal Widen(age: Age, score: Score, c: Boolean) -> Number:
    plan: \"p\"

goal Lists(ages: List<Age>, scores: List<Score>, c: Boolean) -> List<Number>:
    plan: \"p\"

goal Keep(ages: List<Age>, maybe: Age?, age: Age) -> List<Age>:
    plan: \"p\"

goal ToScore(age: Age, ages: List<Age>, person: Person, c: Boolean) -> Score:
    plan: \"p\"

goal ToScores(ages: List<Age>) -> List<Score>:
    plan: \"p\"

goal Rebuild(score: Score) -> Person:
    plan: \"p\"

goal Floor(score: Score?) -> Number:
    plan: \"p\"

goal KeepScore(score: Score?) -> Score:
    plan: \"p\"

goal Labelled(label: Label) -> Label:
    plan: \"p\"

goal Priced(offer: Offer) -> Offer:
    plan: \"p\"
";

fn program() -> Program {
    let (program, diags) = analyze(&SourceFile::new("domains.velme", SOURCE));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(program.goals.iter().position(|g| g.name == name).expect("goal"))
}

/// The compiler's whole IR document of `goal` with `body`, as JSON to edit.
fn document(program: &Program, goal: &str, body: Value) -> Value {
    let id = goal_id(program, goal);
    let target = &program.goals[id.0];
    let (types, domains) = goal_tables(program, id).expect("tables");
    let ty = |t| serde_json::to_value(ir_type(program, t).expect("type")).expect("json");
    let inputs: Vec<Value> = target.params.iter().map(|p| json!([p.name, ty(&p.ty)])).collect();
    json!({
        "ir_version": "0.2", "builtins_version": velme_builtins::BUILTINS_VERSION, "goal": goal,
        "types": types, "domains": domains, "inputs": inputs, "output": ty(&target.output), "body": body,
    })
}

fn run(program: &Program, goal: &str, doc: &Value) -> Result<ValidIr, Vec<Diagnostic>> {
    let request = Request {
        program,
        goal: goal_id(program, goal),
        calls: &[],
        origin: Origin::Complete,
    };
    validate(&doc.to_string(), &request)
}

fn accepts(goal: &str, body: Value) -> ValidIr {
    let program = program();
    run(&program, goal, &document(&program, goal, body)).unwrap_or_else(|d| panic!("{d:#?}"))
}

/// A diagnostic `doc` is rejected with, of `code`, whose note names `path` (any path when empty); every diagnostic
/// has `code`.
fn rejected(goal: &str, doc: &Value, code: Code, path: &str) -> Diagnostic {
    let diags = run(&program(), goal, doc).expect_err("rejected");
    assert!(diags.iter().all(|d| d.code == code), "{diags:#?}");
    diags
        .into_iter()
        .find(|d| path.is_empty() || d.notes.iter().any(|n| n.contains(&format!("`{path}`"))))
        .unwrap_or_else(|| panic!("nothing at {path}"))
}

fn rejects(goal: &str, body: Value, path: &str) -> Diagnostic {
    let program = program();
    rejected(goal, &document(&program, goal, body), Code::IRInvalid, path)
}

fn input(name: &str) -> Value {
    json!({"kind": "input", "name": name})
}

fn number(n: f64) -> Value {
    json!({"kind": "literal", "type": {"t": "Number"}, "value": n})
}

fn text(s: &str) -> Value {
    json!({"kind": "literal", "type": {"t": "Text"}, "value": s})
}

fn branch(then: Value, otherwise: Value) -> Value {
    json!({"kind": "if", "cond": input("c"), "then": then, "else": otherwise})
}

fn numbers(items: Vec<Value>) -> Value {
    json!({"kind": "list", "of": {"t": "Number"}, "items": items})
}

fn edit(mut doc: Value, pointer: &str, with: Value) -> Value {
    *doc.pointer_mut(pointer).unwrap_or_else(|| panic!("{pointer}")) = with;
    doc
}

/// AC-IR-15: the compiler fills a set, Number, Text length and record entry; missing, extra, unknown or altered entries
/// and an unresolved Domain name are `VL0402`; a malformed entry shape is `VL0401`.
#[test]
fn ac_ir_15_the_domains_table_is_the_compilers_and_is_checked() {
    let program = program();
    let person = document(&program, "Older", input("person"));
    assert_eq!(
        to_canonical_string(&person["domains"]).expect("canonical"),
        r#"{"Age":{"base":{"t":"Number"},"lower":{"inclusive":true,"value":18},"upper":{"inclusive":true,"value":120},"whole":true},"Risk":{"base":{"t":"Text"},"members":["LOW","HIGH"]}}"#
    );
    run(&program, "Older", &person).expect("valid");
    let label = document(&program, "Labelled", input("label"));
    assert_eq!(
        to_canonical_string(&label["domains"]).expect("canonical"),
        r#"{"Label":{"base":{"t":"Text"},"length":{"lower":{"inclusive":true,"value":1},"upper":{"inclusive":true,"value":40}}}}"#
    );
    run(&program, "Labelled", &label).expect("valid");
    // A record domain names its own underlying record in `types`; its condition stays in HIR.
    let offer = document(&program, "Priced", input("offer"));
    assert_eq!(
        offer["domains"],
        json!({"Offer": {"base": {"t": "Record", "name": "Offer"}}})
    );
    assert_eq!(offer["inputs"][0][1], json!({"t": "Domain", "name": "Offer"}));
    assert!(offer["types"].get("Offer").is_some());
    run(&program, "Priced", &offer).expect("valid");

    let mut missing = person.clone();
    missing["domains"].as_object_mut().expect("table").remove("Age");
    rejected("Older", &missing, Code::IRInvalid, "/domains");
    let mut extra = person.clone();
    extra["domains"]["Score"] = json!({"base": {"t": "Number"}});
    rejected("Older", &extra, Code::IRInvalid, "/domains/Score");
    let mut unknown = person.clone();
    unknown["domains"]["Ghost"] = json!({"base": {"t": "Number"}});
    rejected("Older", &unknown, Code::IRInvalid, "/domains/Ghost");
    let altered = edit(person.clone(), "/domains/Age/upper/value", json!(121));
    rejected("Older", &altered, Code::IRInvalid, "/domains/Age");
    let unbounded = edit(person.clone(), "/domains/Age/whole", json!(true));
    run(&program, "Older", &unbounded).expect("the same entry");
    let mut no_whole = person.clone();
    no_whole["domains"]["Age"]
        .as_object_mut()
        .expect("entry")
        .remove("whole");
    rejected("Older", &no_whole, Code::IRInvalid, "/domains/Age");
    let field = edit(offer.clone(), "/types/Offer/fields/0/1", json!({"t": "Text"}));
    rejected("Priced", &field, Code::IRInvalid, "/types/Offer");
    let unresolved = edit(
        person.clone(),
        "/types/Person/fields/0/1",
        json!({"t": "Domain", "name": "Agee"}),
    );
    rejected("Older", &unresolved, Code::IRInvalid, "/types/Person/fields/0/1/name");

    for (pointer, with) in [
        ("/domains/Risk/members", json!([])),
        ("/domains/Risk/members", json!(["LOW", "LOW"])),
        (
            "/domains/Risk/length",
            json!({"lower": {"value": 1, "inclusive": true}}),
        ),
        ("/domains/Risk/lower", json!({"value": 1, "inclusive": true})),
        ("/domains/Age/members", json!(["A"])),
        ("/domains/Age/whole", json!(false)),
        ("/domains/Age/lower", Value::Null),
        ("/domains/Age/lower/value", json!("18")),
        ("/domains/Age/base", json!({"t": "Optional", "of": {"t": "Number"}})),
        ("/domains/Age/base", json!({"t": "Domain", "name": "Age"})),
        ("/domains/Age/residual", json!("value != 13")),
    ] {
        let mut doc = person.clone();
        let (parent, key) = pointer.rsplit_once('/').expect("a member");
        doc.pointer_mut(parent).expect("entry")[key] = with;
        rejected("Older", &doc, Code::IRSchemaInvalid, "");
    }
    // A3 (D3 fix round 1): a `length` with no bound is malformed, not a different constraint.
    let empty = edit(label, "/domains/Label/length", json!({}));
    rejected("Labelled", &empty, Code::IRSchemaInvalid, "");
}

/// A2 (D3 fix round 1): the generated schema allows exactly the entry shapes `Deserialize` does: one of four closed
/// shapes, by base, and a `length` with at least one bound. The repo has no JSON Schema validator, so this reads the
/// schema's structure.
#[test]
fn ac_ir_15_the_schema_has_one_closed_shape_per_domain_base() {
    let schema = velme_ir::schema();
    let defs = &schema["$defs"];
    let shapes: Vec<&str> = defs["Domain"]["oneOf"]
        .as_array()
        .expect("oneOf")
        .iter()
        .map(|s| s["$ref"].as_str().expect("a reference"))
        .collect();
    assert_eq!(
        shapes,
        [
            "#/$defs/NumberDomain",
            "#/$defs/TextSetDomain",
            "#/$defs/TextLengthDomain",
            "#/$defs/RecordDomain"
        ]
    );
    let members = |name: &str| {
        let shape = &defs[name];
        assert_eq!(shape["additionalProperties"], json!(false), "{name}");
        assert_eq!(
            shape["properties"]["base"]["additionalProperties"],
            json!(false),
            "{name}"
        );
        let mut keys: Vec<String> = shape["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        (
            keys,
            shape["required"].clone(),
            shape["properties"]["base"]["properties"]["t"]["const"].clone(),
        )
    };
    let strings = |keys: &[&str]| keys.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        members("NumberDomain"),
        (
            strings(&["base", "lower", "upper", "whole"]),
            json!(["base"]),
            json!("Number")
        )
    );
    assert_eq!(
        members("TextSetDomain"),
        (strings(&["base", "members"]), json!(["base", "members"]), json!("Text"))
    );
    assert_eq!(
        members("TextLengthDomain"),
        (strings(&["base", "length"]), json!(["base"]), json!("Text"))
    );
    assert_eq!(
        members("RecordDomain"),
        (strings(&["base"]), json!(["base"]), json!("Record"))
    );
    assert_eq!(defs["Length"]["minProperties"], json!(1));
}

/// AC-IR-16: base-valued outputs, fields and items fill domain positions, through List and Optional; an invalid
/// simple literal is `VL0402`; validation inserts no cast or annotation.
#[test]
fn ac_ir_16_base_values_fill_domain_positions_and_literals_are_checked() {
    let valid = accepts("Ages", input("xs"));
    assert_eq!(
        valid.goal().body,
        from_json_str(&input("xs").to_string()).expect("node")
    );
    accepts("MaybeAges", input("xs"));
    let list = numbers(vec![number(18.0), number(120.0)]);
    let valid = accepts("Ages", list.clone());
    assert_eq!(valid.goal().body, from_json_str(&list.to_string()).expect("node"));
    let person = |age: Value| json!({"kind": "record", "type": "Person", "fields": {"age": age, "risk": text("LOW")}});
    accepts("Older", person(number(30.0)));

    rejects("Pick", number(17.0), "/body");
    rejects("Pick", number(18.5), "/body");
    rejects(
        "Pick",
        json!({"kind": "unary", "op": "neg", "arg": number(20.0)}),
        "/body",
    );
    rejects("Ages", numbers(vec![number(20.0), number(200.0)]), "/body/items/1");
    rejects("Older", person(number(200.0)), "/body/fields/age");
    let risky = json!({"kind": "record", "type": "Person", "fields": {"age": number(30.0), "risk": text("MEDIUM")}});
    rejects("Older", risky, "/body/fields/risk");
    rejects("Labelled", text(""), "/body");
    accepts("Labelled", text("ok"));
}

/// AC-IR-17: a domain widens to its base through List and `?`; a direct expression of another domain fails even with an
/// equal declaration, in an expected branch, field or item too; a join may widen to the base.
#[test]
fn ac_ir_17_domains_widen_to_bases_but_never_to_another_domain() {
    accepts("Widen", input("age"));
    accepts("Lists", input("ages"));
    accepts("Widen", branch(input("age"), input("score")));
    rejects("Pick", input("score"), "/body");
    rejects("Pick", input("twin"), "/body");
    rejects("Pick", branch(input("score"), input("age")), "/body/then");
    rejects("Ages", numbers(vec![input("score")]), "/body/items/0");
    let person = json!({"kind": "record", "type": "Person", "fields": {"age": input("score"), "risk": text("LOW")}});
    rejects("Rebuild", person, "/body/fields/age");
}

/// AC-IR-18: a non-member Text literal compared with a closed-set domain is `VL0402` in either order and under `?`;
/// a member or a computed Text passes; the expected type reaches literals in branches and record literals.
#[test]
fn ac_ir_18_closed_sets_reject_non_member_literals_in_comparisons() {
    let eq = |left: Value, right: Value| json!({"kind": "binary", "op": "eq", "left": left, "right": right});
    rejects("Classify", eq(input("risk"), text("MEDIUM")), "/body/right");
    rejects("Classify", eq(text("MEDIUM"), input("risk")), "/body/left");
    let ne = json!({"kind": "binary", "op": "ne", "left": input("risk"), "right": text("medium")});
    rejects("Classify", ne, "/body/right");
    accepts("Classify", eq(input("risk"), text("HIGH")));
    let computed = json!({"kind": "builtin", "name": "concat", "args": [text("HI"), text("GH")]});
    accepts("Classify", eq(computed, input("risk")));
    let maybe = json!({"kind": "field", "of": input("person"), "field": "risk"});
    let program = program();
    let doc = document(&program, "Older", input("person"));
    let doc = edit(doc, "/output", json!({"t": "Boolean"}));
    let doc = edit(doc, "/body", eq(maybe, text("NONE")));
    let diags = run(&program, "Older", &doc).expect_err("rejected");
    assert!(
        diags
            .iter()
            .any(|d| d.notes.iter().any(|n| n.contains("`/body/right`"))),
        "{diags:#?}"
    );

    rejects("Pick", branch(number(200.0), number(30.0)), "/body/then");
    let record =
        json!({"kind": "literal", "type": {"t": "Record", "name": "Person"}, "value": {"age": 30, "risk": "MID"}});
    rejects("Older", record, "/body");
}

/// AC-IR-19: a Domain in `literal.type` or `list.of` fails the schema; IR 0.1 is rejected; canonical IR keeps member
/// order, sorts domain names and writes exact bounds, with no residual condition.
#[test]
fn ac_ir_19_body_annotations_exclude_domain_and_the_table_is_canonical() {
    let program = program();
    let domain = json!({"t": "Domain", "name": "Age"});
    for body in [
        json!({"kind": "literal", "type": domain, "value": 30}),
        json!({"kind": "literal", "type": {"t": "List", "of": domain}, "value": []}),
        json!({"kind": "list", "of": domain, "items": []}),
        json!({"kind": "list", "of": {"t": "Optional", "of": domain}, "items": []}),
    ] {
        rejected("Ages", &document(&program, "Ages", body), Code::IRSchemaInvalid, "");
    }
    let old = edit(document(&program, "Pick", input("age")), "/ir_version", json!("0.1"));
    rejected("Pick", &old, Code::IRInvalid, "/ir_version");
    let doc = document(&program, "Widen", input("age"));
    let table = to_canonical_string(&doc["domains"]).expect("canonical");
    assert_eq!(
        table,
        r#"{"Age":{"base":{"t":"Number"},"lower":{"inclusive":true,"value":18},"upper":{"inclusive":true,"value":120},"whole":true},"Score":{"base":{"t":"Number"},"lower":{"inclusive":true,"value":0.1}}}"#
    );
    let risk = document(&program, "Classify", input("risk"));
    assert!(
        to_canonical_string(&risk)
            .expect("canonical")
            .contains(r#""members":["LOW","HIGH"]"#)
    );
}

/// AC-IR-20: identity nodes keep a domain, so a kept `Age` doesn't fill a `Score`; operators and fixed-signature
/// built-ins give bases, which do; a type-variable built-in takes a domain; a body list stays base-annotated.
#[test]
fn ac_ir_20_identity_nodes_keep_domains_and_operators_give_bases() {
    let local = |name: &str| json!({"kind": "local", "name": name});
    let truth = json!({"kind": "literal", "type": {"t": "Boolean"}, "value": true});
    let found = json!({"kind": "find", "list": input("ages"), "fn": {"param": "a", "body": truth}});
    for body in [
        input("age"),
        json!({"kind": "let", "bind": [["x", input("age")]], "body": local("x")}),
        branch(input("age"), input("age")),
        json!({"kind": "unwrap_or", "of": found, "default": input("age")}),
        json!({"kind": "field", "of": input("person"), "field": "age"}),
    ] {
        let program = program();
        let diags = run(&program, "ToScore", &document(&program, "ToScore", body)).expect_err("kept Age");
        assert!(diags.iter().all(|d| d.code == Code::IRInvalid), "{diags:#?}");
    }
    for body in [
        json!({"kind": "filter", "list": input("ages"), "fn": {"param": "a", "body": truth}}),
        json!({"kind": "sort_by", "list": input("ages"), "descending": false, "key": {"param": "a", "body": local("a")}}),
    ] {
        rejects("ToScores", body, "/body");
    }
    accepts(
        "ToScore",
        json!({"kind": "binary", "op": "add", "left": input("age"), "right": number(1.0)}),
    );
    accepts("ToScore", json!({"kind": "unary", "op": "neg", "arg": input("age")}));
    accepts(
        "ToScore",
        json!({"kind": "builtin", "name": "round", "args": [input("age")]}),
    );
    let contains = json!({"kind": "builtin", "name": "contains", "args": [input("ages"), input("age")]});
    accepts(
        "ToScore",
        json!({"kind": "if", "cond": contains, "then": number(1.0), "else": number(2.0)}),
    );
    let list = numbers(vec![number(1.0)]);
    let valid = accepts("ToScores", list.clone());
    assert_eq!(valid.goal().body, from_json_str(&list.to_string()).expect("node"));
}

/// AC-IR-21 (typing): joins of distinct domains or a domain and its base widen, through containers; `age == score`
/// compares Numbers; a direct wrong domain in an expected branch fails before the join; a widened computed value
/// validates statically (its membership is the output check's, D4).
#[test]
fn ac_ir_21_joins_widen_and_expected_branches_are_checked_first() {
    accepts("Widen", branch(input("age"), input("score")));
    accepts("Widen", branch(input("age"), number(500.0)));
    accepts("Lists", branch(input("ages"), input("scores")));
    let eq = json!({"kind": "binary", "op": "eq", "left": input("age"), "right": input("score")});
    accepts("Widen", branch_on(eq, input("age")));
    rejects("Pick", branch(input("age"), input("score")), "/body/else");
    accepts(
        "Pick",
        json!({"kind": "binary", "op": "add", "left": input("age"), "right": number(1000.0)}),
    );
    // R2 (D3 fix round 1): a base default outside the optional's domain widens the `unwrap_or` to the base, so it
    // fills a Number output; only where a `Score` is expected is it a non-member literal.
    let floor = json!({"kind": "unwrap_or", "of": input("score"), "default": number(-1.0)});
    accepts("Floor", floor.clone());
    rejects("KeepScore", floor, "/body/default");
    accepts(
        "KeepScore",
        json!({"kind": "unwrap_or", "of": input("score"), "default": number(0.5)}),
    );
}

fn branch_on(cond: Value, then: Value) -> Value {
    json!({"kind": "if", "cond": cond, "then": then.clone(), "else": then})
}
