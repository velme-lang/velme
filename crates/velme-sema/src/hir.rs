//! The typed semantic model (HIR, `compiler/20` §4): resolved ids instead of names, with spans kept (R-CMP-11).

use serde::Serialize;
use velme_diagnostics::Span;
use velme_syntax::ast::{BinaryOp, Quantifier, UnaryOp};

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
    /// The `check` items, each a `Boolean` expression (`language/13` R-CHK-01).
    pub checks: Vec<Expr>,
    /// The `examples`, in order (`language/12` R-GOAL-21).
    pub examples: Vec<Example>,
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

/// `- Goal(args) == expected`, calling the goal it belongs to (R-GOAL-21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Example {
    /// The inputs, one per parameter, each assignable to it.
    pub args: Vec<Expr>,
    /// The expected output, assignable to the goal's output.
    pub expected: Expr,
    /// The whole line after `-`.
    pub span: Span,
}

/// A typed expression of a check or example (`language/13` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Expr {
    /// What the expression is.
    #[serde(flatten)]
    pub kind: ExprKind,
    /// Its type, after narrowing (`language/11` §9).
    pub ty: Type,
    /// The source it came from.
    pub span: Span,
}

/// The forms of [`Expr`]. Surface forms with a built-in meaning are already calls: `x.length` is `length(x)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExprKind {
    /// A number, as its exact decimal text.
    Number {
        /// The text.
        text: String,
    },
    /// Text.
    Text {
        /// The text.
        value: String,
    },
    /// `true` or `false`.
    Bool {
        /// The value.
        value: bool,
    },
    /// `nothing`.
    Nothing,
    /// A goal input, by parameter index.
    Input {
        /// The index into [`Goal::params`].
        index: usize,
    },
    /// A `call` binding, by position in the block.
    Binding {
        /// The binding's index.
        index: usize,
    },
    /// The goal's output.
    Result,
    /// A quantifier variable, by nesting depth: `0` is the outermost quantifier around it.
    Var {
        /// The depth.
        depth: usize,
    },
    /// A built-in call (`language/14`).
    Builtin {
        /// The built-in's catalog name.
        name: &'static str,
        /// The arguments.
        args: Vec<Expr>,
    },
    /// `Type(field: value, …)`, fields in declaration order.
    Record {
        /// The record type.
        ty: TypeId,
        /// One value per field, in declaration order.
        fields: Vec<Expr>,
    },
    /// `[a, b]`.
    List {
        /// The items.
        items: Vec<Expr>,
    },
    /// `record.field`.
    Field {
        /// The record.
        base: Box<Expr>,
        /// The field's index in its record type.
        field: usize,
    },
    /// `records.field` on a list of records: the field of each (R-TYP-14).
    Project {
        /// The list.
        base: Box<Expr>,
        /// The field's index in the element's record type.
        field: usize,
    },
    /// `-x` or `not x`.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// Its operand.
        operand: Box<Expr>,
    },
    /// `a op b`.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// The left operand.
        lhs: Box<Expr>,
        /// The right operand.
        rhs: Box<Expr>,
    },
    /// `x is empty` or `x is not empty` (R-TYP-25).
    IsEmpty {
        /// What is tested.
        operand: Box<Expr>,
        /// `is not empty`.
        negated: bool,
    },
    /// `if a then b`.
    If {
        /// The condition.
        condition: Box<Expr>,
        /// What must hold when it's true.
        then: Box<Expr>,
    },
    /// `every x in xs has p` or `some x in xs has p`.
    Quantified {
        /// `every` or `some`.
        quantifier: Quantifier,
        /// The variable's name, for reports.
        var: String,
        /// The list.
        collection: Box<Expr>,
        /// The condition, where the variable is [`ExprKind::Var`] at this quantifier's depth.
        body: Box<Expr>,
    },
}
