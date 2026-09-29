//! IR types, schema and canonical JSON (`compiler/21` §2–4, §8): the golden IR corpus (R-IR-23) and the criteria of
//! `compiler/21` this crate owns before validation.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::path::Path;

use proptest::prelude::*;
use serde_json::Value;
use velme_ir::{
    CanonicalError, Goal, IR_VERSION, MAX_JSON_DEPTH, Node, ParseError, Type, from_json_str, schema,
    to_canonical_string,
};

fn canonical(json: &str) -> String {
    let value: Value = from_json_str(json).expect("test JSON parses");
    to_canonical_string(&value).expect("test JSON has a canonical form")
}

fn golden(path: &Path) -> Goal {
    let text = std::fs::read_to_string(path).expect("golden file is readable");
    from_json_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn ac_ir_01_committed_schema_matches_generated() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/ir-0.1.json");
    let generated = serde_json::to_string_pretty(&schema()).expect("schema serializes") + "\n";
    if std::env::var_os("VELME_BLESS").is_some() {
        std::fs::write(&path, &generated).expect("schema file is writable");
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale: regenerate it with `VELME_BLESS=1 cargo test -p velme-ir --test ir ac_ir_01` and review the diff",
        path.display()
    );
}

#[test]
fn ac_ir_07_golden_ir_canonical_form() {
    insta::glob!("../../../tests/golden/ir/accept", "*.json", |path| {
        let goal = golden(path);
        assert_eq!(goal.ir_version, IR_VERSION);
        let bytes = to_canonical_string(&goal).expect("golden IR has a canonical form");
        // Canonical form is a fixed point: it parses back and re-serializes to the same bytes.
        let reparsed: Goal = from_json_str(&bytes).expect("canonical IR parses");
        assert_eq!(
            to_canonical_string(&reparsed).expect("canonical IR re-serializes"),
            bytes
        );
        insta::assert_snapshot!(bytes);
    });
}

/// R-IR-23: every node kind of the schema appears in some golden IR file.
#[test]
fn golden_ir_covers_every_node_kind() {
    let schema = schema();
    let kinds: BTreeSet<&str> = schema["$defs"]["Node"]["oneOf"]
        .as_array()
        .expect("Node is a oneOf")
        .iter()
        .map(|variant| {
            variant["properties"]["kind"]["const"]
                .as_str()
                .expect("each variant has a kind")
        })
        .collect();
    let mut seen = BTreeSet::new();
    fn walk(value: &Value, seen: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(kind)) = map.get("kind") {
                    seen.insert(kind.clone());
                }
                map.values().for_each(|v| walk(v, seen));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, seen)),
            _ => {}
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/ir/accept");
    for entry in std::fs::read_dir(dir).expect("golden dir is readable") {
        let goal = golden(&entry.expect("golden dir entry").path());
        walk(&serde_json::to_value(goal).expect("IR serializes"), &mut seen);
    }
    let missing: Vec<_> = kinds.iter().filter(|k| !seen.contains(**k)).collect();
    assert!(
        kinds.len() == 20 && missing.is_empty(),
        "kinds {kinds:?}, missing {missing:?}"
    );
}

#[test]
fn canonical_json_golden_vectors() {
    // R-IR-21's three vectors, each reached from a non-canonical spelling.
    assert_eq!(canonical(r#"{ "é": 2, "b": 1.0, "a": 3 }"#), r#"{"a":3,"b":1,"é":2}"#);
    assert_eq!(
        canonical(r#"{"s": "line1\nline2\u0009tab\"quoteé"}"#),
        "{\"s\":\"line1\\nline2\\ttab\\\"quote\u{e9}\"}"
    );
    assert_eq!(canonical(r#"{"n": 1.0e-1}"#), r#"{"n":0.1}"#);
}

#[test]
fn canonical_strings_escape_only_quote_backslash_and_controls() {
    let raw = "\u{0}\u{8}\u{c}\u{1f}\u{7f}/\u{2028}😀";
    assert_eq!(
        to_canonical_string(raw).expect("text serializes"),
        "\"\\u0000\\b\\f\\u001f\u{7f}/\u{2028}😀\""
    );
}

#[test]
fn canonical_keys_sort_by_code_point() {
    // UTF-16 order (JCS) would put U+1F600 (a surrogate pair) before U+FF61; code point order does not.
    assert_eq!(
        canonical("{\"\u{1f600}\":1,\"\u{ff61}\":2,\"Z\":3,\"a\":4}"),
        "{\"Z\":3,\"a\":4,\"\u{ff61}\":2,\"\u{1f600}\":1}"
    );
}

#[test]
fn canonical_numbers_outside_number_range_are_rejected() {
    let value: Value = from_json_str(r#"{"a": [1, 1e40]}"#).expect("parses");
    assert!(matches!(
        to_canonical_string(&value),
        Err(CanonicalError::NumberOutOfRange { pointer, .. }) if pointer == "/a/1"
    ));
}

#[test]
fn duplicate_object_keys_are_rejected_with_their_pointer() {
    for (json, pointer) in [
        (r#"{"a": 1, "a": 1}"#, "/a"),
        (r#"{"x": [0, {"k": 1, "j": 2, "k": 3}]}"#, "/x/1/k"),
        (r#"{"a/b": {"~": 1, "~": 2}}"#, "/a~1b/~0"),
    ] {
        match from_json_str::<Value>(json) {
            Err(ParseError::DuplicateKey { pointer: p }) => assert_eq!(p, pointer, "{json}"),
            other => panic!("{json}: {other:?}"),
        }
    }
    let ir = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/ir/accept/player_summary.json"),
    )
    .expect("golden file is readable")
    .replacen(
        r#""rank":  {"kind": "local""#,
        r#""score": {"kind": "input", "name": "player"}, "rank": {"kind": "local""#,
        1,
    );
    assert!(matches!(
        from_json_str::<Goal>(&ir),
        Err(ParseError::DuplicateKey { pointer }) if pointer == "/body/fields/score"
    ));
}

#[test]
fn unknown_fields_and_kinds_do_not_parse() {
    for json in [
        r#"{"kind": "local", "name": "x", "extra": 1}"#,
        r#"{"kind": "loop", "body": {"kind": "local", "name": "x"}}"#,
        r#"{"kind": "unary", "op": "sqrt", "arg": {"kind": "local", "name": "x"}}"#,
        r#"{"kind": "literal", "type": {"t": "Optional", "of": {"t": "Number"}, "x": 1}, "value": 1}"#,
        r#"{"kind": "literal", "type": {"t": "Number", "x": 1}, "value": 1}"#,
        r#"{"kind": "list", "of": {"t": "Nothing", "extra": null}, "items": []}"#,
        r#"{"kind": "map", "list": {"kind": "input", "name": "xs"}, "fn": {"acc": "a", "param": "p", "body": {"kind": "local", "name": "p"}}}"#,
    ] {
        assert!(
            matches!(from_json_str::<Node>(json), Err(ParseError::Json(_))),
            "{json}"
        );
    }
}

#[test]
fn reserved_number_keys_in_input_are_rejected() {
    // serde_json passes exact numbers as a map under this key; written in the input it must not decode as a number.
    for (json, pointer) in [
        (
            r#"{"$serde_json::private::Number": "5"}"#,
            "/$serde_json::private::Number",
        ),
        (
            r#"{"v": [{"\u0024serde_json::private::Number": "5"}]}"#,
            "/v/0/$serde_json::private::Number",
        ),
        (
            r#"{"$serde_json::private::RawValue": "5"}"#,
            "/$serde_json::private::RawValue",
        ),
    ] {
        match from_json_str::<Value>(json) {
            Err(ParseError::ReservedKey { pointer: p }) => assert_eq!(p, pointer, "{json}"),
            other => panic!("{json}: {other:?}"),
        }
    }
    let literal = r#"{"kind": "literal", "type": {"t": "Number"}, "value": {"$serde_json::private::Number": "5"}}"#;
    assert!(matches!(
        from_json_str::<Node>(literal),
        Err(ParseError::ReservedKey { .. })
    ));
    // Genuine numbers, and keys spelled with escapes, still work (and still count as duplicates).
    assert_eq!(canonical(r#"{"n": 5, "m": [1.5e3, -0]}"#), r#"{"m":[1500,0],"n":5}"#);
    assert!(matches!(
        from_json_str::<Value>(r#"{"a": 1, "\u0061": 2}"#),
        Err(ParseError::DuplicateKey { pointer }) if pointer == "/a"
    ));
}

#[test]
fn nested_optional_types_are_one_optional() {
    // §2.1: `Optional(Optional(T))` is `Optional(T)`, read and written, so both spellings hash alike.
    let flat = Type::Optional {
        of: Box::new(Type::Number {}),
    };
    let nested: Type =
        from_json_str(r#"{"t": "Optional", "of": {"t": "Optional", "of": {"t": "Optional", "of": {"t": "Number"}}}}"#)
            .expect("nested optional parses");
    assert_eq!(nested, flat);
    let built = Type::List {
        of: Box::new(Type::Optional {
            of: Box::new(flat.clone()),
        }),
    };
    assert_eq!(
        to_canonical_string(&built).expect("type serializes"),
        r#"{"of":{"of":{"t":"Number"},"t":"Optional"},"t":"List"}"#
    );
}

/// IR at the §7 expression depth (128), in the shape that nests JSON deepest per node (`let` binds, three levels
/// each), parses; stage 7 of the validator, not the parser, owns that limit.
#[test]
fn ir_at_the_expression_depth_limit_parses() {
    const IR_DEPTH: usize = 128;
    let mut body =
        r#"{"kind": "literal", "type": {"t": "List", "of": {"t": "Optional", "of": {"t": "Number"}}}, "value": [1]}"#
            .to_owned();
    for _ in 1..IR_DEPTH {
        body = format!(r#"{{"kind": "let", "bind": [["x", {body}]], "body": {{"kind": "local", "name": "x"}}}}"#);
    }
    let ir = format!(
        r#"{{"ir_version": "0.1", "builtins_version": "0.1", "goal": "Deep", "types": {{}}, "inputs": [],
            "output": {{"t": "List", "of": {{"t": "Optional", "of": {{"t": "Number"}}}}}}, "body": {body}}}"#
    );
    let goal: Goal = from_json_str(&ir).expect("IR at the depth limit parses");
    to_canonical_string(&goal).expect("IR at the depth limit serializes");
    // A chain of `unary` nodes up to the JSON guard parses too, on a default test thread's stack (R-IR-18).
    let depth = MAX_JSON_DEPTH - 1;
    let chain = r#"{"kind": "unary", "op": "neg", "arg": "#.repeat(depth)
        + r#"{"kind": "input", "name": "x"}"#
        + &"}".repeat(depth);
    from_json_str::<Node>(&chain).expect("a chain inside the guard parses");
}

#[test]
fn json_deeper_than_the_guard_is_rejected() {
    let nested = |depth: usize| "[".repeat(depth) + &"]".repeat(depth);
    from_json_str::<Value>(&nested(MAX_JSON_DEPTH)).expect("JSON at the guard parses");
    assert!(matches!(
        from_json_str::<Value>(&nested(MAX_JSON_DEPTH + 1)),
        Err(ParseError::TooDeep { .. })
    ));
    assert!(matches!(
        from_json_str::<Node>(&nested(100_000)),
        Err(ParseError::TooDeep { .. })
    ));
}

#[test]
fn call_nodes_parse_anywhere_so_the_validator_can_reject_them() {
    // AC-IR-02 is the validator's (VL0402); this is its precondition: the node is in the schema, not a VL0401.
    let node: Node =
        from_json_str(r#"{"kind": "call", "binding": "b", "goal": "G", "goal_signature": "b3:00", "args": []}"#)
            .expect("a call node parses as an expression");
    assert!(matches!(node, Node::Call(_)));
}

#[test]
fn empty_calls_serialize_like_omitted_calls() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/ir/accept");
    let leaf = std::fs::read_to_string(dir.join("find_badge.json")).expect("golden file is readable");
    let with_empty = leaf.replacen(r#""body":"#, r#""calls": [], "body":"#, 1);
    let a: Goal = from_json_str(&leaf).expect("parses");
    let b: Goal = from_json_str(&with_empty).expect("parses");
    assert_eq!(to_canonical_string(&a), to_canonical_string(&b));
}

fn json_number() -> impl Strategy<Value = String> {
    (
        any::<bool>(),
        0u64..1_000_000,
        proptest::option::of(0u32..10_000),
        -8i32..8,
    )
        .prop_map(|(neg, int, frac, exp)| {
            let sign = if neg { "-" } else { "" };
            let frac = frac.map(|f| format!(".{f:04}")).unwrap_or_default();
            format!("{sign}{int}{frac}e{exp}")
        })
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        json_number().prop_map(|n| serde_json::from_str(&n).expect("generated number is JSON")),
        any::<String>().prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::btree_map(any::<String>(), inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    /// CC-TEST-05: canonical form is a fixed point of parse + serialize.
    #[test]
    fn canonical_form_is_a_fixed_point(value in json_value()) {
        let once = to_canonical_string(&value).expect("generated values are in range");
        let reparsed: Value = from_json_str(&once).expect("canonical JSON parses");
        prop_assert_eq!(to_canonical_string(&reparsed).expect("reparsed value is in range"), once);
    }
}
