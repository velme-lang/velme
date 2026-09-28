//! The typed semantic model (HIR, `compiler/20` §4): resolved ids instead of names, with spans kept (R-CMP-11).

use serde::Serialize;
use velme_diagnostics::Span;

/// A record type: an index into [`Program::types`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TypeId(pub usize);

/// A goal: an index into [`Program::goals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct GoalId(pub usize);

/// A checked file (`compiler/20` §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Program {
    /// The language version from the header, or the one assumed without one (R-SYN-15).
    pub language_version: String,
    /// Record types, in source order.
    pub types: Vec<RecordType>,
    /// Goals, in source order.
    pub goals: Vec<Goal>,
}

impl Program {
    /// The record type `id`.
    pub fn record(&self, id: TypeId) -> Option<&RecordType> {
        self.types.get(id.0)
    }

    /// `ty` as a learner writes it: `List<Player?>`.
    pub fn type_name(&self, ty: &Type) -> String {
        ty.display(&self.types)
    }
}

/// `type Name:` (`language/11` §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordType {
    /// Its name.
    pub name: String,
    /// Its fields, in declaration order (R-TYP-23).
    pub fields: Vec<FieldDef>,
    /// The whole declaration.
    pub span: Span,
}

impl RecordType {
    /// The field called `name`.
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// A record field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldDef {
    /// Its name.
    pub name: String,
    /// Its type.
    pub ty: Type,
    /// The field's line.
    pub span: Span,
}

/// A v0.1 type (`language/11` §2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    /// `Number`
    Number,
    /// `Text`
    Text,
    /// `Boolean`
    Boolean,
    /// `Nothing`, the type of the literal `nothing`.
    Nothing,
    /// `T?`
    Optional(Box<Type>),
    /// `List<T>`
    List(Box<Type>),
    /// A record type.
    Record(TypeId),
    /// A type that already has an error; it matches everything so the error isn't repeated (`compiler/20` R-CMP-10).
    /// Never present in a program returned by [`crate::analyze`].
    Error,
}

impl Type {
    /// `self` as a learner writes it, naming records from `types`.
    pub fn display(&self, types: &[RecordType]) -> String {
        match self {
            Type::Number => "Number".to_owned(),
            Type::Text => "Text".to_owned(),
            Type::Boolean => "Boolean".to_owned(),
            Type::Nothing => "Nothing".to_owned(),
            Type::Optional(inner) => format!("{}?", inner.display(types)),
            Type::List(element) => format!("List<{}>", element.display(types)),
            Type::Record(id) => types.get(id.0).map_or_else(|| "?".to_owned(), |r| r.name.clone()),
            Type::Error => "?".to_owned(),
        }
    }
}

/// `goal Name(…) -> T:` (`language/12` §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Goal {
    /// Its name.
    pub name: String,
    /// Its parameters, in order.
    pub params: Vec<Param>,
    /// Its output type.
    pub output: Type,
    /// The plan text, normalized (D-21), if it has one.
    pub plan: Option<String>,
    /// The whole declaration.
    pub span: Span,
}

/// A goal parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Param {
    /// Its name.
    pub name: String,
    /// Its type.
    pub ty: Type,
    /// `name: type`.
    pub span: Span,
}
