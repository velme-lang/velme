//! Builders of IR nodes as JSON, for the tests that write a goal's body by hand.

use serde_json::{Value as Json, json};

/// The type `Number`.
pub fn number() -> Json {
    json!({"t": "Number"})
}

/// The type `List<Number>`.
pub fn numbers() -> Json {
    json!({"t": "List", "of": number()})
}

/// A read of the input `name`.
pub fn input(name: &str) -> Json {
    json!({"kind": "input", "name": name})
}

/// A read of the local `name`.
pub fn local(name: &str) -> Json {
    json!({"kind": "local", "name": name})
}

/// A `Number` literal.
pub fn literal(value: i64) -> Json {
    json!({"kind": "literal", "type": number(), "value": value})
}

/// `left op right`.
pub fn binary(op: &str, left: Json, right: Json) -> Json {
    json!({"kind": "binary", "op": op, "left": left, "right": right})
}

/// A call of the builtin `name`.
pub fn builtin(name: &str, args: &[Json]) -> Json {
    json!({"kind": "builtin", "name": name, "args": args})
}
