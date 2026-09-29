//! The value-size function of memory accounting (`runtime/30` §7.1, D-53, D-83), defined once so the interpreter and
//! the WASM backend charge the same numbers (R-SBX-05). Every per-kind cost is in [`value_bytes`], [`list_bytes`] and
//! [`record_bytes`]. A list or record is sized when it is built and keeps its size, so parts it shares are charged in
//! full without being walked again.

use crate::Value;

/// Bytes of a `Number`.
pub const NUMBER_BYTES: u64 = 16;

/// Bytes of a `Boolean`.
pub const BOOLEAN_BYTES: u64 = 8;

/// Bytes the tag of a `T?` takes, before its `T`.
pub const OPTIONAL_BYTES: u64 = 8;

/// Bytes of a `Text`, and of a `List` or record, before their contents.
pub const HEADER_BYTES: u64 = 16;

/// The bytes of a `Text` of `bytes` bytes: 16 + ⌈bytes/8⌉ · 8, the 8-byte slots of the WASM ABI.
pub fn text_bytes(bytes: usize) -> u64 {
    let slots = u64::try_from(bytes.div_ceil(8)).unwrap_or(u64::MAX);
    HEADER_BYTES.saturating_add(slots.saturating_mul(8))
}

/// The bytes of `range(length)`: a list of `length` numbers.
pub fn number_list_bytes(length: u64) -> u64 {
    HEADER_BYTES.saturating_add(length.saturating_mul(NUMBER_BYTES))
}

/// The logical size of `value`, whatever its parts share (D-83): Number 16, Boolean 8, Nothing 0, Text 16 + ⌈bytes/8⌉ · 8,
/// List 16 + Σ items, record 16 + Σ fields, an item or field of optional type 8 more, saturating at `u64::MAX`. Constant time: a list or record knows its size.
pub fn value_bytes(value: &Value) -> u64 {
    match value {
        Value::Number(_) => NUMBER_BYTES,
        Value::Boolean(_) => BOOLEAN_BYTES,
        Value::Nothing => 0,
        Value::Text(text) => text_bytes(text.len()),
        Value::List(list) => list.bytes(),
        Value::Record(record) => record.bytes(),
    }
}

/// What creating `value` allocates (D-89): its [`value_bytes`] if it is a Text, List or Record, and nothing for a
/// scalar, which lives in no linear memory of its own. A scalar inside a List or Record is part of that value's size.
pub fn charged_bytes(value: &Value) -> u64 {
    match value {
        Value::Number(_) | Value::Boolean(_) | Value::Nothing => 0,
        Value::Text(_) | Value::List(_) | Value::Record(_) => value_bytes(value),
    }
}

/// [`value_bytes`], by its older name.
pub fn size_of(value: &Value) -> u64 {
    value_bytes(value)
}

/// The size of an item or field of a List or Record: its own size, and 8 more if its type is optional, present or not
/// (`T?` is 8 + `T`, and `nothing` is 0 + the 8).
fn slot_bytes(value: &Value, optional: bool) -> u64 {
    value_bytes(value).saturating_add(if optional { OPTIONAL_BYTES } else { 0 })
}

/// The size of a list of `items`, of an optional item type if `optional`: 16 + Σ items.
pub fn list_bytes(items: &[Value], optional: bool) -> u64 {
    items.iter().fold(HEADER_BYTES, |total, item| {
        total.saturating_add(slot_bytes(item, optional))
    })
}

/// The size of a record of `fields`, each with whether its type is optional: 16 + Σ fields.
pub fn record_bytes<'a>(fields: impl Iterator<Item = (&'a Value, bool)>) -> u64 {
    fields.fold(HEADER_BYTES, |total, (field, optional)| {
        total.saturating_add(slot_bytes(field, optional))
    })
}
