//! The reply schema (`compiler/22` R-SYNTH-10) and the one-line-per-node summary of it that the prompt carries
//! (R-SYNTH-35).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde_json::{Map, Value, json};

/// The IR schema, generated once.
static IR_SCHEMA: LazyLock<Value> = LazyLock::new(velme_ir::schema);

/// The JSON Schema a reply must satisfy: one IR goal or one question object (R-SYNTH-10). The IR schema
/// (`compiler/21` R-IR-20) becomes the `IrGoal` definition beside its own, so its `$ref`s keep resolving.
pub fn reply_schema() -> Value {
    let mut goal = IR_SCHEMA.clone();
    let mut defs = match goal.as_object_mut().and_then(|root| root.remove("$defs")) {
        Some(Value::Object(defs)) => defs,
        _ => Map::new(),
    };
    if let Some(root) = goal.as_object_mut() {
        root.remove("$schema");
    }
    defs.insert("IrGoal".to_owned(), goal);
    defs.insert(
        "Question".to_owned(),
        json!({
            "description": "A question, only when the plan leaves open a choice that changes the result (R-SYNTH-32).",
            "type": "object",
            "additionalProperties": false,
            "properties": {"question": {"type": "string", "description": "The question, in plain words."}},
            "required": ["question"],
        }),
    );
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SynthReply",
        "$defs": defs,
        "oneOf": [{"$ref": "#/$defs/IrGoal"}, {"$ref": "#/$defs/Question"}],
    })
}

/// One line per IR node kind and one for the question object: the kind, its fields and what it means (R-SYNTH-35).
/// Generated from the IR schema, so it cannot drift from it; part of `prompt_version` (D-97).
pub fn schema_summary() -> &'static [String] {
    static SUMMARY: LazyLock<Vec<String>> = LazyLock::new(summary);
    &SUMMARY
}

/// The compact alias table of `reply_format = "compact"` (R-SYNTH-36), part of `prompt_version` (D-97). Empty until the
/// compact format exists (M5b).
pub fn alias_table() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::new()
}

fn summary() -> Vec<String> {
    let schema = IR_SCHEMA.clone();
    let variants = schema
        .pointer("/$defs/Node/oneOf")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut lines: Vec<String> = variants
        .iter()
        .filter_map(|variant| {
            let kind = variant.pointer("/properties/kind/const")?.as_str()?;
            let fields: Vec<&str> = variant
                .get("properties")?
                .as_object()?
                .keys()
                .map(String::as_str)
                .filter(|name| *name != "kind")
                .collect();
            let what = variant.get("description").and_then(Value::as_str).unwrap_or_default();
            Some(format!("- {kind} {{{}}}: {}", fields.join(", "), one_line(what)))
        })
        .collect();
    lines.push(
        r#"- question {question}: ask only when the plan leaves open a choice that changes the result"#.to_owned(),
    );
    lines
}

/// `text` with every run of whitespace as one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
