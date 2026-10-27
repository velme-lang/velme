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
const SOURCE: &str = "language: velme/0.2

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
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/synth-request-0.2.json");
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

/// An `external` request for `ir-json` is the request as it was; for `compact` it names the format and carries that
/// format's reply schema, a body under the alias or a question, not the IR schema (`compiler/22` §3.2, D-156).
#[test]
fn ac_synth_57_an_external_compact_request_carries_the_compact_reply_schema() {
    let base = request("FindBadge");
    assert_eq!(base.clone().for_external(velme_synth::ReplyFormat::IrJson), base);

    let compact = base.clone().for_external(velme_synth::ReplyFormat::Compact);
    let text = velme_ir::to_canonical_string(&compact).expect("canonical");
    assert!(text.contains(r#""reply_format":"compact""#), "{text}");
    let schema = &compact.output_schema;
    assert!(schema.pointer("/$defs").is_none() && schema["anyOf"].as_array().map(Vec::len) == Some(2));
    let key = velme_synth::alias_table().keys["body"].clone();
    assert!(schema["anyOf"][0]["required"] == serde_json::json!([key]), "{schema}");
    // The committed request is a good deal smaller for it.
    let plain = velme_ir::to_canonical_string(&base).expect("canonical");
    assert!(text.len() + 10_000 < plain.len(), "{} vs {}", text.len(), plain.len());
}

/// D2 review #3: a record-domain parameter, also under `List` and `?`, brings its record into the request's types.
#[test]
fn a_record_domain_parameter_reaches_the_request_types() {
    let source = "language: velme/0.2

type Offer where value.price >= 0:
    price: Number

goal Best(offers: List<Offer?>, first: Offer) -> Number:
    plan: \"Return the best price.\"
";
    let program = program(source);
    let request = build_request(&program, goal_id(&program, "Best"), source).expect("a request");
    let names: Vec<&str> = request.types.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Offer"]);
}

/// A program whose goals reach a closed set, a Number domain, a record domain and, only through that domain's
/// condition, a record its constructor builds; comments sit inside the declarations.
const DOMAINS: &str = "language: velme/0.2

type Risk = Text one of \"LOW\", \"HIGH\"  # the bureau's two bands
type Age = Number where value >= 18 and value <= 120
type Floor:
    amount: Number
type Offer where value.price >= Floor(amount: 0).amount:
    # in cents
    price: Number
    risk: Risk
type Plain:
    name: Text

goal Quote(age: Age, offer: Offer) -> Risk:
    plan: \"Rate the offer.\"

goal Name(plain: Plain) -> Text:
    plan: \"Return the name.\"
";

/// AC-SYNTH-62: the request and prompt carry each reachable declaration as written, comments stripped, a record domain
/// once in declaration form (its underlying record stays in `types` for assembly) and a record only a condition
/// constructs; a goal that reaches a domain alone gets the base-in-body rule line.
#[test]
fn ac_synth_62_types_are_full_declarations_and_domain_goals_get_the_rule() {
    let program = program(DOMAINS);
    let request = build_request(&program, goal_id(&program, "Quote"), DOMAINS).expect("a request");
    let declarations: Vec<(&str, &str)> = request
        .declarations
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    assert_eq!(
        declarations,
        [
            ("Age", "type Age = Number where value >= 18 and value <= 120"),
            ("Floor", "type Floor:\n    amount: Number"),
            (
                "Offer",
                "type Offer where value.price >= Floor(amount: 0).amount:\n    price: Number\n    risk: Risk"
            ),
            ("Risk", "type Risk = Text one of \"LOW\", \"HIGH\""),
        ]
    );
    // The request's table is the IR goal's (D-166); `Floor`, reached only through a constructor, isn't in it.
    let (_, domains) = velme_ir::goal_tables(&program, goal_id(&program, "Quote")).expect("tables");
    assert_eq!(request.domains, domains);
    assert_eq!(
        request.domains.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Age", "Offer", "Risk"]
    );
    let names: Vec<&str> = request.types.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Offer"]);
    let prompt = velme_synth::render(&request).expect("a prompt");
    let types = prompt.task.split("Types:\n").nth(1).expect("a Types section");
    let types = types.split("\n\nPlan:").next().expect("the section");
    assert_eq!(
        types,
        [
            "type Age = Number where value >= 18 and value <= 120",
            "type Floor:\n    amount: Number",
            "type Offer where value.price >= Floor(amount: 0).amount:\n    price: Number\n    risk: Risk",
            "type Risk = Text one of \"LOW\", \"HIGH\"",
            "Declared types are their base in the body.",
        ]
        .join("\n")
    );
    let plain = build_request(&program, goal_id(&program, "Name"), DOMAINS).expect("a request");
    assert!(plain.declarations.is_empty() && plain.domains.is_empty());
    let prompt = velme_synth::render(&plain).expect("a prompt");
    assert!(!prompt.task.contains("Declared types"), "{}", prompt.task);
    assert!(prompt.task.contains("type Plain = { name: Text }"), "{}", prompt.task);
}

/// AC-SYNTH-62: only the body of a reply is read; the request's `domains`, the compiler's own table, completes the IR
/// (D-166).
#[test]
fn ac_synth_62_the_compiler_table_completes_a_reply() {
    let program = program(DOMAINS);
    let id = goal_id(&program, "Quote");
    let request = build_request(&program, id, DOMAINS).expect("a request");
    let (_, domains) = velme_ir::goal_tables(&program, id).expect("tables");
    let body = json!({"kind": "literal", "type": {"t": "Text"}, "value": "LOW"});
    let assembled: Value = serde_json::from_str(&request.assemble(&body).expect("assembled")).expect("json");
    assert_eq!(assembled["domains"], serde_json::to_value(&domains).expect("table"));
    assert_eq!(
        assembled["domains"]["Risk"],
        json!({"base": {"t": "Text"}, "members": ["LOW", "HIGH"]})
    );
    let request_ir = velme_ir::Request {
        program: &program,
        goal: id,
        calls: &[],
        origin: velme_ir::Origin::Candidate,
    };
    velme_ir::validate(&assembled.to_string(), &request_ir).expect("valid");
}

/// AC-SYNTH-63: the request is version 0.2 against IR and built-ins of this build, with `domains` and `declarations`
/// required in the committed schema (D-166) and empty when no domain is reached.
#[test]
fn ac_synth_63_the_request_is_version_0_2_with_its_declarations() {
    let request = request("FindBadge");
    assert_eq!(
        (
            request.request_version.as_str(),
            request.ir_version.as_str(),
            request.builtins_version.as_str()
        ),
        ("0.2", velme_ir::IR_VERSION, velme_builtins::BUILTINS_VERSION)
    );
    assert_eq!(REQUEST_VERSION, "0.2");
    assert!(request.domains.is_empty() && request.declarations.is_empty());
    let schema = request_schema();
    for member in ["domains", "declarations"] {
        let required = schema["required"].as_array().expect("required");
        assert!(required.contains(&json!(member)), "{member}: {schema}");
        let mut without = serde_json::to_value(&request).expect("json");
        without.as_object_mut().expect("object").remove(member);
        assert!(
            velme_ir::from_json_str::<SynthRequest>(&without.to_string()).is_err(),
            "{member}"
        );
    }
}
