//! A small JSON Schema validator for the one contract this repository publishes, `docs/schemas/velme-cli-1.schema.json`
//! (`tooling/40` R-CLI-15, AC-CLI-12). It knows only the keywords that schema uses and panics on any other, so the schema
//! can't grow a keyword this checker would silently skip. No dependency: a full validator would bring a large tree for one
//! file.

use serde_json::Value;

/// Keywords that annotate and constrain nothing.
const ANNOTATIONS: [&str; 5] = ["$schema", "$id", "title", "description", "$defs"];

/// Where the published schema is, from the repository root.
const CLI_SCHEMA: &str = "docs/schemas/velme-cli-1.schema.json";

/// Fails the calling test unless `envelope` is valid against `docs/schemas/velme-cli-1.schema.json` (AC-CLI-12).
pub fn assert_cli_envelope(envelope: &Value) {
    let schema: Value = serde_json::from_str(&crate::read(&crate::repo(CLI_SCHEMA))).expect("the schema is JSON");
    let found = violations(&schema, envelope);
    assert!(
        found.is_empty(),
        "the envelope breaks {CLI_SCHEMA}:\n{}",
        found.join("\n")
    );
}

/// Every way `instance` fails `schema`, as `path: reason` lines; empty when it is valid. `$ref` names a `$defs` entry of the
/// root `schema`.
pub fn violations(schema: &Value, instance: &Value) -> Vec<String> {
    let mut out = Vec::new();
    check(schema, schema, instance, "$", &mut out);
    out
}

fn check(root: &Value, schema: &Value, instance: &Value, path: &str, out: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        panic!("a schema is an object: {schema}");
    };
    for (keyword, rule) in schema {
        match keyword.as_str() {
            k if ANNOTATIONS.contains(&k) => {}
            "$ref" => {
                let name = rule
                    .as_str()
                    .and_then(|r| r.strip_prefix("#/$defs/"))
                    .unwrap_or_default();
                match root.get("$defs").and_then(|defs| defs.get(name)) {
                    Some(target) => check(root, target, instance, path, out),
                    None => panic!("$ref {rule} names no definition"),
                }
            }
            "type" => {
                let ok = match rule.as_str().unwrap_or_default() {
                    "object" => instance.is_object(),
                    "array" => instance.is_array(),
                    "string" => instance.is_string(),
                    "boolean" => instance.is_boolean(),
                    "null" => instance.is_null(),
                    "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
                    other => panic!("unknown type {other}"),
                };
                if !ok {
                    out.push(format!("{path}: should be {rule}, but is {instance}"));
                }
            }
            "const" if instance != rule => out.push(format!("{path}: should be {rule}, but is {instance}")),
            "enum" if !rule.as_array().is_some_and(|options| options.contains(instance)) => {
                out.push(format!("{path}: should be one of {rule}, but is {instance}"));
            }
            "minimum" => {
                let min = rule.as_i64().unwrap_or_default();
                if instance.as_i64().is_some_and(|n| n < min) {
                    out.push(format!("{path}: should be at least {min}, but is {instance}"));
                }
            }
            "pattern" => {
                let pattern = rule.as_str().unwrap_or_default();
                if instance.as_str().is_some_and(|text| !matches(pattern, text)) {
                    out.push(format!("{path}: should match {pattern}, but is {instance}"));
                }
            }
            "required" => {
                if let Some(object) = instance.as_object() {
                    for name in rule.as_array().into_iter().flatten().filter_map(Value::as_str) {
                        if !object.contains_key(name) {
                            out.push(format!("{path}: `{name}` is missing"));
                        }
                    }
                }
            }
            "properties" => {
                if let Some(object) = instance.as_object() {
                    for (name, sub) in rule.as_object().into_iter().flatten() {
                        if let Some(value) = object.get(name) {
                            check(root, sub, value, &format!("{path}.{name}"), out);
                        }
                    }
                }
            }
            "items" => {
                for (i, item) in instance.as_array().into_iter().flatten().enumerate() {
                    check(root, rule, item, &format!("{path}[{i}]"), out);
                }
            }
            "oneOf" => {
                let passing = rule
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|option| {
                        let mut inner = Vec::new();
                        check(root, option, instance, path, &mut inner);
                        inner.is_empty()
                    })
                    .count();
                if passing != 1 {
                    out.push(format!(
                        "{path}: should match exactly one alternative, but matches {passing}"
                    ));
                }
            }
            "anyOf" => {
                let any = rule.as_array().into_iter().flatten().any(|option| {
                    let mut inner = Vec::new();
                    check(root, option, instance, path, &mut inner);
                    inner.is_empty()
                });
                if !any {
                    out.push(format!(
                        "{path}: should match at least one alternative, but matches none"
                    ));
                }
            }
            "const" | "enum" => {}
            other => panic!("the validator doesn't know the keyword `{other}`"),
        }
    }
}

/// Whether `text` matches `pattern`, of the form `^` then literal characters and `[class]{n}` runs, then `$`: all the
/// published schema uses.
fn matches(pattern: &str, text: &str) -> bool {
    let body = pattern
        .strip_prefix('^')
        .and_then(|p| p.strip_suffix('$'))
        .unwrap_or_else(|| panic!("unsupported pattern {pattern}"));
    let (mut rest, mut chars) = (body, text.chars());
    while !rest.is_empty() {
        if let Some(class) = rest.strip_prefix('[') {
            let (set, after) = class
                .split_once("]{")
                .unwrap_or_else(|| panic!("unsupported pattern {pattern}"));
            let (count, after) = after
                .split_once('}')
                .unwrap_or_else(|| panic!("unsupported pattern {pattern}"));
            let count: usize = count
                .parse()
                .unwrap_or_else(|_| panic!("unsupported pattern {pattern}"));
            let ranges: Vec<char> = set.chars().collect();
            let allowed = |c: char| {
                let mut i = 0;
                while i < ranges.len() {
                    if ranges.get(i + 1) == Some(&'-') {
                        if ranges
                            .get(i)
                            .is_some_and(|lo| ranges.get(i + 2).is_some_and(|hi| (*lo..=*hi).contains(&c)))
                        {
                            return true;
                        }
                        i += 3;
                    } else {
                        if ranges.get(i) == Some(&c) {
                            return true;
                        }
                        i += 1;
                    }
                }
                false
            };
            for _ in 0..count {
                if !chars.next().is_some_and(allowed) {
                    return false;
                }
            }
            rest = after;
        } else {
            let mut literal = rest.chars();
            let Some(want) = literal.next() else { break };
            if chars.next() != Some(want) {
                return false;
            }
            rest = literal.as_str();
        }
    }
    chars.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_subset_of_json_schema_the_contract_uses_is_checked() {
        let schema = json!({
            "$defs": {"code": {"type": "string", "pattern": "^VL[0-9]{4}$"}},
            "type": "object",
            "required": ["code", "items"],
            "properties": {
                "code": {"$ref": "#/$defs/code"},
                "items": {"type": "array", "items": {"enum": ["a", "b"]}},
                "n": {"type": "integer", "minimum": 1},
                "either": {"oneOf": [{"type": "string"}, {"type": "null"}]},
                "any": {"anyOf": [{"type": "string"}, {}]}
            }
        });
        assert!(
            violations(
                &schema,
                &json!({"code": "VL0101", "items": ["a"], "n": 1, "either": null, "any": "x"})
            )
            .is_empty()
        );
        let bad = violations(&schema, &json!({"code": "VL01", "items": ["c"], "n": 0, "either": 3}));
        assert_eq!(bad.len(), 4, "{bad:?}");
        assert_eq!(violations(&schema, &json!({"items": []})), ["$: `code` is missing"]);
    }
}
