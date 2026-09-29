//! Runtime values (`runtime/30` R-RUN-01): immutable and structurally compared.

use std::sync::Arc;

use crate::Number;
use crate::memory::{list_bytes, record_bytes};

/// A Velme value (R-RUN-01). A present value of a `T?` is the `T` value itself; `nothing` is [`Value::Nothing`].
///
/// Equality is structural (`language/11` R-TYP-20): lists by length and items, records field by field, numbers
/// numerically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// A `Number`.
    Number(Number),
    /// A `Text`.
    Text(Arc<str>),
    /// A `Boolean`.
    Boolean(bool),
    /// `nothing`.
    Nothing,
    /// A `List<T>`.
    List(List),
    /// A record.
    Record(Arc<Record>),
}

/// A list value: its items, shared by every copy, and its logical size in bytes, worked out once when it is built
/// (`runtime/30` §7.1), so charging memory for it never walks it again. Two lists are equal when their items are, whatever
/// the static type their size was worked out for.
#[derive(Debug, Clone)]
pub struct List(Arc<Items>);

#[derive(Debug)]
struct Items {
    bytes: u64,
    /// Whether the items are of an optional type, which costs each 8 bytes more (§7.1).
    optional: bool,
    items: Vec<Value>,
}

impl List {
    /// The logical size of the list (§7.1).
    pub fn bytes(&self) -> u64 {
        self.0.bytes
    }

    /// Whether the list's item type is optional, so a list made of some of its items has the same.
    pub fn optional_items(&self) -> bool {
        self.0.optional
    }
}

impl PartialEq for List {
    fn eq(&self, other: &List) -> bool {
        self.0.items == other.0.items
    }
}

impl Eq for List {}

impl std::hash::Hash for List {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.items.hash(state);
    }
}

impl std::ops::Deref for List {
    type Target = [Value];

    fn deref(&self) -> &[Value] {
        &self.0.items
    }
}

/// A record value: its type's name and its fields in declaration order (R-RUN-01, R-TYP-23).
#[derive(Debug, Clone)]
pub struct Record {
    /// The record type's name.
    pub name: String,
    /// The fields, in declaration order.
    pub fields: Vec<(String, Value)>,
    /// Its logical size, worked out when it was built (§7.1).
    bytes: u64,
}

impl PartialEq for Record {
    fn eq(&self, other: &Record) -> bool {
        self.name == other.name && self.fields == other.fields
    }
}

impl Eq for Record {}

impl std::hash::Hash for Record {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.fields.hash(state);
    }
}

impl Record {
    /// The logical size of the record (§7.1).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The field called `name`.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

impl Value {
    /// A `Text` value.
    pub fn text(text: &str) -> Value {
        Value::Text(text.into())
    }

    /// A `List` value whose item type is not optional.
    pub fn list(items: Vec<Value>) -> Value {
        Value::list_of(items, false)
    }

    /// A `List` value whose item type is optional if `optional`, which the static types say (`runtime/30` §7.1).
    pub fn list_of(items: Vec<Value>, optional: bool) -> Value {
        Value::List(List(Arc::new(Items {
            bytes: list_bytes(&items, optional),
            optional,
            items,
        })))
    }

    /// A record value none of whose fields is optional.
    pub fn record(name: &str, fields: Vec<(String, Value)>) -> Value {
        let optional = vec![false; fields.len()];
        Value::record_of(name, fields, &optional)
    }

    /// A record value whose `i`th field is of an optional type if `optional[i]`, which the record type says.
    pub fn record_of(name: &str, fields: Vec<(String, Value)>, optional: &[bool]) -> Value {
        Value::Record(Arc::new(Record {
            name: name.to_owned(),
            bytes: record_bytes(fields.iter().map(|(_, value)| value).zip(optional.iter().copied())),
            fields,
        }))
    }
}

impl From<Number> for Value {
    fn from(n: Number) -> Value {
        Value::Number(n)
    }
}
