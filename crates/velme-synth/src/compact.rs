//! The compact reply format (`compiler/22` R-SYNTH-36): the IR tree with every property name and node `kind` tag replaced
//! by a short alias from a fixed table, generated from the IR schema so it is versioned with it. A pure renaming:
//! [`expand`] gives canonical IR before validation, so the validator, fingerprints, artifacts and lock only ever see
//! canonical IR (D-21).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::Serialize;
use serde_json::{Map, Value};

/// The fixed alias table: canonical name to alias, for property names and for node kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AliasTable {
    /// Property names.
    pub keys: BTreeMap<String, String>,
    /// Node `kind` tags.
    pub kinds: BTreeMap<String, String>,
}

/// A compact reply used a name that is no alias of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownAlias;

static TABLE: LazyLock<AliasTable> = LazyLock::new(build);

/// The alias table (R-SYNTH-36), part of `prompt_version` (D-97).
pub fn table() -> &'static AliasTable {
    &TABLE
}

fn build() -> AliasTable {
    let schema = velme_ir::schema();
    let mut keys = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    collect(&schema, &mut keys, &mut kinds);
    AliasTable {
        keys: aliases(&keys),
        kinds: aliases(&kinds),
    }
}

/// Every property name under `schema`, and every `kind` constant.
fn collect(schema: &Value, keys: &mut BTreeSet<String>, kinds: &mut BTreeSet<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get("properties") {
                keys.extend(properties.keys().cloned());
                if let Some(kind) = properties
                    .get("kind")
                    .and_then(|k| k.get("const"))
                    .and_then(Value::as_str)
                {
                    kinds.insert(kind.to_owned());
                }
            }
            for value in map.values() {
                collect(value, keys, kinds);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, keys, kinds);
            }
        }
        _ => {}
    }
}

/// Each name in sorted order takes its shortest prefix that no earlier name took.
fn aliases(names: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut taken = BTreeSet::new();
    let mut table = BTreeMap::new();
    for name in names {
        let chars: Vec<char> = name.chars().collect();
        let alias = (1..=chars.len())
            .map(|n| chars.iter().take(n).collect::<String>())
            .find(|candidate| !taken.contains(candidate))
            .unwrap_or_else(|| name.clone());
        taken.insert(alias.clone());
        table.insert(name.clone(), alias);
    }
    table
}

fn reverse(table: &BTreeMap<String, String>) -> BTreeMap<&str, &str> {
    table
        .iter()
        .map(|(name, alias)| (alias.as_str(), name.as_str()))
        .collect()
}

/// Where a name's object keys are the program's own, not the IR's: the record types of a goal, a record node's fields.
fn is_dynamic(key: &str, value: &Value) -> bool {
    matches!(key, "types" | "fields") && value.is_object()
}

/// Whether `object` is a `literal` node, whose `value` is data and never renamed.
fn is_literal(object: &Map<String, Value>) -> bool {
    object.get("kind").and_then(Value::as_str) == Some("literal")
}

/// `value`, a canonical IR document, written with aliases.
pub fn compress(value: &Value) -> Value {
    let table = table();
    walk(value, &|key| table.keys.get(key).map(String::as_str), &|kind| {
        table.kinds.get(kind).map(String::as_str)
    })
}

/// A JSON Pointer or path of canonical names, written with aliases.
pub fn compress_path(path: &str) -> String {
    let table = table();
    path.split('/')
        .map(|segment| table.keys.get(segment).map_or(segment, String::as_str))
        .collect::<Vec<_>>()
        .join("/")
}

fn walk<'a>(value: &Value, key: &dyn Fn(&str) -> Option<&'a str>, kind: &dyn Fn(&str) -> Option<&'a str>) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|item| walk(item, key, kind)).collect()),
        Value::Object(map) => {
            let literal = is_literal(map);
            Value::Object(
                map.iter()
                    .map(|(name, inner)| {
                        let renamed = key(name).unwrap_or(name).to_owned();
                        let inner = if is_dynamic(name, inner) {
                            match inner {
                                Value::Object(members) => Value::Object(
                                    members
                                        .iter()
                                        .map(|(k, v)| (k.clone(), walk(v, key, kind)))
                                        .collect::<Map<String, Value>>(),
                                ),
                                other => other.clone(),
                            }
                        } else if literal && name == "value" {
                            inner.clone()
                        } else if name == "kind" {
                            match inner.as_str().and_then(kind) {
                                Some(alias) => Value::String(alias.to_owned()),
                                None => inner.clone(),
                            }
                        } else {
                            walk(inner, key, kind)
                        };
                        (renamed, inner)
                    })
                    .collect::<Map<String, Value>>(),
            )
        }
        other => other.clone(),
    }
}

/// `value`, a compact reply, as canonical IR; a name that is no alias is [`UnknownAlias`].
pub fn expand(value: &Value) -> Result<Value, UnknownAlias> {
    let table = table();
    let (keys, kinds) = (reverse(&table.keys), reverse(&table.kinds));
    let kind_alias = table.keys.get("kind").map_or("kind", String::as_str);
    unwalk(value, &keys, &kinds, kind_alias)
}

fn unwalk(
    value: &Value,
    keys: &BTreeMap<&str, &str>,
    kinds: &BTreeMap<&str, &str>,
    kind_alias: &str,
) -> Result<Value, UnknownAlias> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| unwalk(item, keys, kinds, kind_alias))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let literal = map
                .get(kind_alias)
                .and_then(Value::as_str)
                .and_then(|alias| kinds.get(alias))
                .is_some_and(|name| *name == "literal");
            let mut out = Map::new();
            for (alias, inner) in map {
                let name = keys.get(alias.as_str()).ok_or(UnknownAlias)?;
                let inner = if is_dynamic(name, inner) {
                    match inner {
                        Value::Object(members) => Value::Object(
                            members
                                .iter()
                                .map(|(k, v)| Ok((k.clone(), unwalk(v, keys, kinds, kind_alias)?)))
                                .collect::<Result<Map<String, Value>, UnknownAlias>>()?,
                        ),
                        other => other.clone(),
                    }
                } else if literal && *name == "value" {
                    inner.clone()
                } else if *name == "kind" {
                    let alias = inner.as_str().ok_or(UnknownAlias)?;
                    Value::String((*kinds.get(alias).ok_or(UnknownAlias)?).to_owned())
                } else {
                    unwalk(inner, keys, kinds, kind_alias)?
                };
                out.insert((*name).to_owned(), inner);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// The table as the output contract states it (R-SYNTH-36): `kind=k, body=b, …`.
pub fn describe() -> String {
    let table = table();
    let list = |map: &BTreeMap<String, String>| {
        map.iter()
            .map(|(name, alias)| format!("{name}={alias}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Write the reply in the compact format: the same tree, with each property name and each node kind replaced by \
         its short name. Property names: {}. Node kinds: {}. Everything else, including the `t` tags of types, \
         is written as usual.",
        list(&table.keys),
        list(&table.kinds)
    )
}
