//! The reply schema (`compiler/22` R-SYNTH-10, D-103) and the one-line-per-node summary of it that the prompt carries
//! (R-SYNTH-35).

use std::sync::LazyLock;

use serde_json::{Map, Value, json};

/// The IR schema, generated once.
static IR_SCHEMA: LazyLock<Value> = LazyLock::new(velme_ir::schema);

/// The JSON Schema a reply must satisfy: `{"body": <expression>}` or one question object (R-SYNTH-10, D-103). Only the
/// definitions an expression reaches are kept, so the model is shown nothing of the envelope Velme writes itself.
pub fn reply_schema() -> Value {
    let all = match IR_SCHEMA.get("$defs") {
        Some(Value::Object(defs)) => defs.clone(),
        _ => Map::new(),
    };
    let mut defs = Map::new();
    let mut pending = vec!["Node".to_owned()];
    while let Some(name) = pending.pop() {
        if defs.contains_key(&name) {
            continue;
        }
        if let Some(def) = all.get(&name) {
            let mut def = def.clone();
            if name == "Node" {
                // A `call` node is valid only in the compiler's `calls` (R-IR-02): a body reads a call's result as a
                // `local`, so the model is never offered it.
                if let Some(Value::Array(variants)) = def.get_mut("oneOf") {
                    variants.retain(|variant| !is_call(variant));
                }
            }
            refs(&def, &mut pending);
            defs.insert(name, def);
        }
    }
    defs.insert(
        "Body".to_owned(),
        json!({
            "description": "The goal's body: one expression node.",
            "type": "object",
            "additionalProperties": false,
            "properties": {"body": {"$ref": "#/$defs/Node"}},
            "required": ["body"],
        }),
    );
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
        "oneOf": [{"$ref": "#/$defs/Body"}, {"$ref": "#/$defs/Question"}],
    })
}

/// The operator names of a `binary` node and of a `unary` node, from the IR schema (D-104).
pub(crate) fn operators() -> (Vec<String>, Vec<String>) {
    let names = |def: &str| -> Vec<String> {
        IR_SCHEMA
            .pointer(&format!("/$defs/{def}/oneOf"))
            .and_then(Value::as_array)
            .map(|variants| {
                variants
                    .iter()
                    .filter_map(|v| v.get("const").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    (names("BinaryOperator"), names("UnaryOperator"))
}

/// Whether the `Node` variant `variant` is the `call` node.
fn is_call(variant: &Value) -> bool {
    variant.pointer("/properties/kind/const").and_then(Value::as_str) == Some("call")
}

/// The names of the `$defs` that `schema` refers to.
fn refs(schema: &Value, into: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
            {
                into.push(name.to_owned());
            }
            map.values().for_each(|inner| refs(inner, into));
        }
        Value::Array(items) => items.iter().for_each(|inner| refs(inner, into)),
        _ => {}
    }
}

/// One line per IR node kind and one for the question object: the kind, its fields and what it means (R-SYNTH-35).
/// Generated from the IR schema, so it cannot drift from it; part of `prompt_version` (D-97).
pub fn schema_summary() -> &'static [String] {
    static SUMMARY: LazyLock<Vec<String>> = LazyLock::new(summary);
    &SUMMARY
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
        .filter(|variant| !is_call(variant))
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

#[cfg(test)]
mod tests {
    use super::operators;

    /// Both operator lists come out of the schema, and hold the operators the prompt and the hints name.
    #[test]
    fn the_operator_lists_are_read_from_the_schema() {
        let (binary, unary) = operators();
        assert!(binary.iter().any(|op| op == "add"), "{binary:?}");
        assert!(unary.iter().any(|op| op == "not"), "{unary:?}");
    }
}
