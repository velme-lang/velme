//! The synthesis request, its schema and the reply schema (`compiler/22` §3.1, §4, R-SYNTH-10).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;

use serde_json::{Value, json};
use velme_diagnostics::Code;
use velme_synth::{
    REQUEST_VERSION, SynthRequest, TaskKind, build_request, reply_schema, request_schema, schema_summary,
};
use velme_test_support::{goal_id, program, read, repo};

/// The golden goals: a leaf with a check and examples, and a composite over two children.
const SOURCE: &str = "language: velme/0.1

type Player:
    name: Text
    score: Number

goal CalculateScore(player: Player) -> Number:
    plan: \"Return the player's score.\"

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise.
    check:
        - result == \"Gold\" or result == \"Silver\" or result == \"Bronze\"
    examples:
        - FindBadge(Player(name: \"Lina\", score: 820)) == \"Silver\"

goal Summary(player: Player) -> Text:
    call:
        score = CalculateScore(player)
        badge = FindBadge(player)
    plan: \"Join the badge and the score.\"
    check:
        - result != \"\"

goal Wired(player: Player) -> Number:
    call:
        result = CalculateScore(player)
";

fn request(goal: &str) -> SynthRequest {
    let program = program(SOURCE);
    build_request(&program, goal_id(&program, goal), SOURCE).expect("a request")
}

/// The request, with the reply schema (checked on its own) left out of the snapshot.
fn shown(request: &SynthRequest) -> Value {
    let mut shown = serde_json::to_value(request).expect("serializes");
    shown["output_schema"] = json!("<reply schema>");
    shown
}

fn pretty(request: &SynthRequest) -> String {
    serde_json::to_string_pretty(&shown(request)).expect("prints")
}

/// A leaf request holds the signature, reachable types, plan, checks with their lowered IR, examples with literal
/// values, the budget and the builtins (`compiler/22` §4); a composite one adds the call bindings, with the children's
/// signatures and never their IR.
#[test]
fn a_request_is_the_structured_form_of_the_prompt_table() {
    insta::assert_snapshot!("leaf_request", pretty(&request("FindBadge")));
    insta::assert_snapshot!("composite_request", pretty(&request("Summary")));
    let leaf = request("FindBadge");
    assert_eq!(
        (leaf.task, leaf.request_version.as_str()),
        (TaskKind::Leaf, REQUEST_VERSION)
    );
    assert_eq!(request("Summary").task, TaskKind::Composite);
    assert_eq!(leaf.output_schema, reply_schema());
}

/// Nothing is synthesized for a wired goal (D-4).
#[test]
fn a_wired_goal_has_no_request() {
    let program = program(SOURCE);
    let error = build_request(&program, goal_id(&program, "Wired"), SOURCE).expect_err("no request");
    assert_eq!(error.code, Code::InternalError);
}

/// The request hash is the BLAKE3 of the canonical JSON: the same for the same request, different for another (R-SYNTH-43).
#[test]
fn the_request_hash_follows_the_request() {
    let first = request("FindBadge");
    assert_eq!(first.hash().expect("hash"), request("FindBadge").hash().expect("hash"));
    let mut edited = first.clone();
    edited.plan.push_str(" Also, be kind.");
    assert_ne!(first.hash().expect("hash"), edited.hash().expect("hash"));
    assert!(first.hash().expect("hash").to_string().starts_with("b3:"));
}

/// The committed `velme-synth-request` schema is the generated one (`compiler/22` §3.2, like AC-IR-01), and a message
/// round-trips through it.
#[test]
fn the_committed_request_schema_matches_the_generated_one() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/synth-request-0.1.json");
    let generated = serde_json::to_string_pretty(&request_schema()).expect("schema serializes") + "\n";
    if std::env::var_os("VELME_BLESS").is_some() {
        std::fs::write(&path, &generated).expect("schema file is writable");
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale: regenerate it with `VELME_BLESS=1 cargo test -p velme-synth --test request` and review the diff",
        path.display()
    );
    let request = request("FindBadge");
    let text = velme_ir::to_canonical_string(&request).expect("canonical");
    let back: SynthRequest = velme_ir::from_json_str(&text).expect("parses");
    assert_eq!(back, request);
    assert_eq!(request.request_version, REQUEST_VERSION);
}

/// The reply is one IR goal or one question object (R-SYNTH-10), and every `$ref` in the schema resolves.
#[test]
fn the_reply_schema_accepts_an_ir_goal_or_a_question() {
    let schema = reply_schema();
    assert_eq!(schema["oneOf"].as_array().map(Vec::len), Some(2));
    fn refs(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(target)) = map.get("$ref") {
                    out.push(target.clone());
                }
                map.values().for_each(|v| refs(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    refs(&schema, &mut found);
    assert!(found.len() > 4);
    for target in found {
        let pointer = target.strip_prefix('#').expect("a local reference");
        assert!(schema.pointer(pointer).is_some(), "{target} doesn't resolve");
    }
    assert_eq!(schema["$defs"]["Question"]["required"], json!(["question"]));
}

/// The schema summary has one line per IR node kind a body may hold, which is every kind but `call`, plus the question
/// object (R-SYNTH-35, D-103).
#[test]
fn the_schema_summary_has_a_line_per_node_kind() {
    let lines = schema_summary();
    let ir = velme_ir::schema();
    let kinds = ir["$defs"]["Node"]["oneOf"].as_array().expect("Node is a oneOf").len();
    assert_eq!(lines.len(), kinds - 1 + 1);
    assert!(!lines.iter().any(|l| l.starts_with("- call ")));
    assert!(lines.iter().any(|l| l.starts_with("- literal {")));
    assert!(lines.last().is_some_and(|l| l.starts_with("- question")));
    // Nothing in the golden corpus is needed to read them.
    let _ = read(&repo("examples/beginner/add.velme"));
}
