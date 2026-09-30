//! The generated test inputs (`compiler/22` §7, D-96): the golden vectors V1–V3, which a conforming generator
//! reproduces exactly, and its determinism.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::Value;
use velme_ir::Fingerprint;
use velme_synth::{TestInputs, test_inputs};
use velme_test_support::{goal_id, program};

fn generate(source: &str, goal: &str, key: &str, examples: &[Vec<Value>]) -> TestInputs {
    let program = program(source);
    let key: Fingerprint = key.parse().expect("a fingerprint");
    test_inputs(&program, goal_id(&program, goal), key, examples).expect("inputs")
}

fn lines(inputs: &TestInputs) -> Vec<&str> {
    inputs.inputs.iter().map(|i| i.json.as_str()).collect()
}

const V1: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
";

const V2: &str = "language: velme/0.1

goal Greet(name: Text, loud: Boolean) -> Text:
    plan: \"Greet.\"
    examples:
        - Greet(\"Lina\", false) == \"Hello, Lina\"
";

const V3: &str = "language: velme/0.1

type Item:
    name: Text
    price: Number

goal Total(items: List<Item>, bonus: Number?) -> Number:
    plan: \"Total.\"
";

/// V1 of `compiler/22` §7.4: six boundary inputs, no pairwise ones (all duplicates), 58 random.
#[test]
fn ac_synth_37_golden_vector_v1() {
    let inputs = generate(
        V1,
        "Double",
        "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        &[],
    );
    assert_eq!(
        (inputs.examples, inputs.boundary, inputs.pairwise, inputs.random),
        (0, 6, 0, 58)
    );
    assert_eq!(
        lines(&inputs)[..13],
        [
            r#"{"n":0}"#,
            r#"{"n":1}"#,
            r#"{"n":-1}"#,
            r#"{"n":0.5}"#,
            r#"{"n":1000000}"#,
            r#"{"n":-1000000}"#,
            r#"{"n":-502.7}"#,
            r#"{"n":-756}"#,
            r#"{"n":-710}"#,
            r#"{"n":95.63}"#,
            r#"{"n":-739}"#,
            r#"{"n":-343}"#,
            r#"{"n":-908.11}"#,
        ]
    );
}

/// V2: one example, four boundary, two pairwise, 57 random; the seed is 0.
#[test]
fn ac_synth_37_golden_vector_v2() {
    let example = vec![Value::text("Lina"), Value::Boolean(false)];
    let inputs = generate(
        V2,
        "Greet",
        "b3:0000000000000000000000000000000000000000000000000000000000000000",
        &[example],
    );
    assert_eq!(
        (inputs.examples, inputs.boundary, inputs.pairwise, inputs.random),
        (1, 4, 2, 57)
    );
    assert_eq!(
        lines(&inputs)[..11],
        [
            r#"{"loud":false,"name":"Lina"}"#,
            r#"{"loud":true,"name":""}"#,
            r#"{"loud":false,"name":"a"}"#,
            r#"{"loud":true,"name":"Lina"}"#,
            r#"{"loud":false,"name":"é🙂"}"#,
            r#"{"loud":false,"name":""}"#,
            r#"{"loud":true,"name":"a"}"#,
            r#"{"loud":false,"name":"wmvzeyimsmd"}"#,
            r#"{"loud":true,"name":"g0en"}"#,
            r#"{"loud":false,"name":"ArobAoxfrwvpdzh"}"#,
            r#"{"loud":true,"name":"ur0e🙂"}"#,
        ]
    );
}

/// V3: records and lists and an optional; seven boundary inputs, ten pairwise, 47 random.
#[test]
fn ac_synth_37_golden_vector_v3() {
    let inputs = generate(
        V3,
        "Total",
        "b3:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        &[],
    );
    assert_eq!(
        (inputs.examples, inputs.boundary, inputs.pairwise, inputs.random),
        (0, 7, 10, 47)
    );
    let all = lines(&inputs);
    assert_eq!(
        all[..4],
        [
            r#"{"bonus":null,"items":[]}"#,
            r#"{"bonus":0,"items":[{"name":"","price":0}]}"#,
            r#"{"bonus":1,"items":[{"name":"","price":0},{"name":"a","price":1},{"name":"Lina","price":-1}]}"#,
            r#"{"bonus":-1,"items":[{"name":"","price":0},{"name":"","price":0}]}"#,
        ]
    );
    assert_eq!(
        all[7..11],
        [
            r#"{"bonus":0,"items":[]}"#,
            r#"{"bonus":1,"items":[]}"#,
            r#"{"bonus":-1,"items":[]}"#,
            r#"{"bonus":null,"items":[{"name":"","price":0}]}"#,
        ]
    );
    assert_eq!(
        all[17],
        r#"{"bonus":878,"items":[{"name":"b","price":985.59},{"name":"ézqdojntxafulf","price":482},{"name":"ZAe ","price":172},{"name":"lymgcelohwqbgv","price":-898}]}"#
    );
}

/// The inputs are a pure function of the goal and its `contract_key` (AC-SYNTH-06, R-SYNTH-17).
#[test]
fn ac_synth_06_generated_inputs_are_byte_identical_across_runs() {
    let key = "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let a = generate(V3, "Total", key, &[]);
    let b = generate(V3, "Total", key, &[]);
    assert_eq!(a, b);
    let other = generate(V3, "Total", &key.replace("af13", "bf13"), &[]);
    assert_ne!(
        lines(&a),
        lines(&other),
        "another contract key gives other random inputs"
    );
    assert_eq!(a.inputs.len(), 64);
}
