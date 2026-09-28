//! The typed semantic model (HIR, `compiler/20` §4): resolved ids instead of names, with spans kept (R-CMP-11).

use serde::Serialize;
use velme_builtins::limits;
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
    /// What is synthesized for it (`language/12` §2, D-4).
    pub kind: GoalKind,
    /// The limits of its own invocation (R-GOAL-20).
    pub budget: Budget,
    /// The `call` bindings, in block order; a wired goal's last one is `result` (R-GOAL-12).
    pub bindings: Vec<Binding>,
    /// The plan text, normalized (D-21), if it has one.
    pub plan: Option<String>,
    /// The `check` items, each a `Boolean` expression (`language/13` R-CHK-01).
    pub checks: Vec<Expr>,
    /// The `examples`, in order (`language/12` R-GOAL-21).
    pub examples: Vec<Example>,
    /// The whole declaration.
    pub span: Span,
}

/// What is synthesized for a goal (`language/12` §2, `compiler/20` §3 phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalKind {
    /// No `call` block: the whole body is synthesized.
    Leaf,
    /// A `call` block without `result`: only the tail is synthesized (D-5).
    Composite,
    /// A `call` block ending in `result`: nothing is synthesized (D-4).
    Wired,
}

/// The effective limits of one goal's own invocation: the system caps, lowered by its `budget` line (R-GOAL-20,
/// `runtime/30` R-RUN-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Budget {
    /// Fuel for this invocation (`cpu=` × `FUEL_PER_MS`).
    pub max_fuel: u64,
    /// Bytes allocated by this invocation (`memory=`).
    pub max_memory: u64,
    /// Goal invocations in this goal's subtree, itself included (`calls=`).
    pub max_goal_calls: u64,
    /// Call depth below this goal (`depth=`).
    pub max_call_depth: u64,
}

impl Budget {
    /// The system caps (`runtime/30` §7), for a goal without a `budget` line.
    pub const SYSTEM: Budget = Budget {
        max_fuel: limits::MAX_FUEL,
        max_memory: limits::MAX_MEMORY,
        max_goal_calls: limits::MAX_GOAL_CALLS,
        max_call_depth: limits::MAX_CALL_DEPTH,
    };
}

/// `name = Goal(args)` in a `call` block (`language/12` §3, R-GOAL-14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    /// The bound name, `result` for a wired goal's last binding.
    pub name: String,
    /// The goal called.
    pub callee: GoalId,
    /// One argument per callee parameter: a path from an input or an earlier binding, or a literal (R-GOAL-08).
    pub args: Vec<Expr>,
    /// The callee's output type (R-GOAL-12).
    pub ty: Type,
    /// When it can run: 1 + the latest wave among the bindings it uses; inputs are wave 0 (R-GOAL-14).
    pub wave: usize,
    /// The whole line.
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

/// A typed expression of a check, an example or a call argument (`language/13` §3, `language/12` R-GOAL-08).
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
    /// A `call` binding.
    Binding {
        /// The index into [`Goal::bindings`].
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
