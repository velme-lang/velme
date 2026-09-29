//! Runtime values (`runtime/30` R-RUN-01): immutable and structurally compared.

use std::sync::Arc;

use crate::Number;

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
    List(Arc<[Value]>),
    /// A record.
    Record(Arc<Record>),
}

/// A record value: its type's name and its fields in declaration order (R-RUN-01, R-TYP-23).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Record {
    /// The record type's name.
    pub name: String,
    /// The fields, in declaration order.
    pub fields: Vec<(String, Value)>,
}

impl Record {
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

    /// A `List` value.
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(items.into())
    }

    /// A record value.
    pub fn record(name: &str, fields: Vec<(String, Value)>) -> Value {
        Value::Record(Arc::new(Record {
            name: name.to_owned(),
            fields,
        }))
    }
}

impl From<Number> for Value {
    fn from(n: Number) -> Value {
        Value::Number(n)
    }
}
