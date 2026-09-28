//! The abstract syntax tree (`language/10` §4). Every node carries its byte span (D-70); the golden corpus
//! serializes it as JSON.

use serde::Serialize;
use velme_diagnostics::Span;

use crate::Unit;

/// A name as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ident {
    /// The name.
    pub name: String,
    /// Where it is.
    pub span: Span,
}

/// A parsed file: the declarations that parsed, and the ones that didn't (R-SYN-17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Program {
    /// The `language:` line, if the file has one (R-SYN-15).
    pub header: Option<Header>,
    /// Declarations holding no error, in source order; warnings don't count (D-76).
    pub decls: Vec<Decl>,
    /// Declarations that failed to parse or hold an error; sema skips them and suppresses follow-on errors (R-SYN-17).
    pub failed: Vec<FailedDecl>,
    /// The whole file.
    pub span: Span,
}

/// `language: velme/0.1` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Header {
    /// The language name; must be `velme`.
    pub language: Ident,
    /// The version as written, compared as text (R-SYN-21).
    pub version: Version,
    /// The whole line.
    pub span: Span,
}

/// A header `VERSION`, kept as text (R-SYN-21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Version {
    /// The version as written, e.g. `0.1`.
    pub text: String,
    /// Where it is.
    pub span: Span,
}

/// A declaration that failed to parse (R-SYN-17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedDecl {
    /// Its name, if the parser got that far.
    pub name: Option<Ident>,
    /// The whole declaration.
    pub span: Span,
}

/// A top-level declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decl {
    /// `type Name:` with fields.
    Type(TypeDecl),
    /// `goal Name(…) -> T:` with a body.
    Goal(Box<GoalDecl>),
}

/// `type_decl` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeDecl {
    /// The type's name.
    pub name: Ident,
    /// Its fields, in order.
    pub fields: Vec<Field>,
    /// The whole declaration.
    pub span: Span,
}

/// `field_decl` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    /// The field's name.
    pub name: Ident,
    /// Its type.
    pub ty: TypeExpr,
    /// The whole line.
    pub span: Span,
}

/// `type_expr` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeExpr {
    /// The type before any `?`.
    #[serde(flatten)]
    pub base: BaseType,
    /// Whether it ends in `?`.
    pub optional: bool,
    /// The whole type.
    pub span: Span,
}

/// `base_type` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaseType {
    /// A type name, resolved by sema.
    Named {
        /// The name.
        name: Ident,
    },
    /// `List<T>`.
    List {
        /// The element type.
        element: Box<TypeExpr>,
    },
}

/// `goal_decl` (§4). Body blocks are at most once each (R-SYN-14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalDecl {
    /// The goal's name.
    pub name: Ident,
    /// Its parameters.
    pub params: Vec<Param>,
    /// Its output type.
    pub output: TypeExpr,
    /// `budget …`
    pub budget: Option<Budget>,
    /// `call:`
    pub call: Option<CallBlock>,
    /// `plan:`
    pub plan: Option<Plan>,
    /// `check:`
    pub check: Option<CheckBlock>,
    /// `examples:`
    pub examples: Option<ExamplesBlock>,
    /// The whole declaration.
    pub span: Span,
}

/// `param` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Param {
    /// The parameter's name.
    pub name: Ident,
    /// Its type.
    pub ty: TypeExpr,
    /// `name: type`.
    pub span: Span,
}

/// `budget_line` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Budget {
    /// The items, in order.
    pub items: Vec<BudgetItem>,
    /// The whole line.
    pub span: Span,
}

/// `budget_item` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetItem {
    /// What is limited.
    pub name: Ident,
    /// The limit.
    pub value: Number,
    /// Its unit, written right after the number.
    pub unit: Option<UnitLit>,
    /// `name = value unit`.
    pub span: Span,
}

/// A `UNIT` after a budget number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitLit {
    /// The unit.
    pub unit: Unit,
    /// Where it is.
    pub span: Span,
}

/// A checked `NUMBER` (R-SYN-03): digits with underscores removed, `-` in front if negated in a literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Number {
    /// The exact decimal text, e.g. `-1000.50`.
    pub text: String,
    /// Where it is, including a literal's `-`.
    pub span: Span,
}

/// `call_block` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallBlock {
    /// The bindings, in order.
    pub bindings: Vec<Binding>,
    /// The whole block.
    pub span: Span,
}

/// `binding` (§4). `result` may name only the last one (R-GOAL-23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    /// The bound name, possibly `result`.
    pub name: Ident,
    /// The goal called.
    pub callee: Ident,
    /// Its arguments.
    pub args: Vec<CallArg>,
    /// The whole line.
    pub span: Span,
}

/// `call_arg` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "arg", rename_all = "snake_case")]
pub enum CallArg {
    /// A parameter, earlier binding or field of one.
    Path(Path),
    /// A literal value.
    Literal(Literal),
}

/// `path` (§4): `name.field.field`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Path {
    /// The names, first to last.
    pub segments: Vec<Ident>,
    /// The whole path.
    pub span: Span,
}

/// `plan_block` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The plan text; a block plan is already normalized (D-21, D-67).
    pub text: String,
    /// How it was written.
    pub form: PlanForm,
    /// The plan text's span.
    pub span: Span,
}

/// Inline `plan: "…"` or block `plan: |`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanForm {
    /// `plan: "…"`
    Inline,
    /// `plan: |`
    Block,
}

/// `check_block` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckBlock {
    /// One expression per `- …` line.
    pub items: Vec<Expr>,
    /// The whole block.
    pub span: Span,
}

/// `examples_block` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExamplesBlock {
    /// The examples, in order.
    pub items: Vec<Example>,
    /// The whole block.
    pub span: Span,
}

/// `example_item` (§4): `- Goal(args) == expected`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Example {
    /// The goal called.
    pub goal: Ident,
    /// Its arguments.
    pub args: Vec<Literal>,
    /// The expected output.
    pub expected: Literal,
    /// The whole line after `-`.
    pub span: Span,
}

/// `literal` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Literal {
    /// What the literal is.
    #[serde(flatten)]
    pub kind: LiteralKind,
    /// The whole literal.
    pub span: Span,
}

/// The forms of `literal` (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiteralKind {
    /// A number, possibly negative.
    Number {
        /// The exact decimal text.
        text: String,
    },
    /// Text, escapes decoded.
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
    /// `[a, b]`.
    List {
        /// The items.
        items: Vec<Literal>,
    },
    /// `Type(field: value, …)`.
    Record {
        /// The type's name.
        name: Ident,
        /// The fields, as written.
        fields: Vec<FieldValue>,
    },
}

/// `name: literal` inside a record literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldValue {
    /// The field's name.
    pub name: Ident,
    /// Its value.
    pub value: Literal,
    /// `name: value`.
    pub span: Span,
}

/// `expr` (§4, precedence §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Expr {
    /// What the expression is.
    #[serde(flatten)]
    pub kind: ExprKind,
    /// The whole expression.
    pub span: Span,
}

/// The forms of `expr` (§4). Parentheses leave no node; the tree shape records them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExprKind {
    /// A number.
    Number {
        /// The exact decimal text.
        text: String,
    },
    /// Text, escapes decoded.
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
    /// `result`.
    Result,
    /// A name: parameter, binding or quantifier variable.
    Name {
        /// The name.
        name: String,
    },
    /// `Name(args)`: a built-in call or record literal; sema decides (R-SYN-16).
    Call {
        /// The name called.
        callee: Ident,
        /// The arguments; all named or all positional.
        args: Vec<Arg>,
    },
    /// `[a, b]`.
    List {
        /// The items.
        items: Vec<Expr>,
    },
    /// `base.field`.
    Field {
        /// The expression before the `.`.
        base: Box<Expr>,
        /// The field.
        field: Ident,
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
    /// `x is empty` or `x is not empty`.
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
        /// The variable.
        var: Ident,
        /// The list.
        collection: Box<Expr>,
        /// The condition.
        body: Box<Expr>,
    },
}

/// `arg` (§4): `[name:] expr`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Arg {
    /// The field name, for a record literal.
    pub name: Option<Ident>,
    /// The value.
    pub value: Expr,
    /// The whole argument.
    pub span: Span,
}

/// Prefix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    /// `-`
    Neg,
    /// `not`
    Not,
}

/// Infix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    /// `or`
    Or,
    /// `and`
    And,
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

/// `every` or `some`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantifier {
    /// `every`
    Every,
    /// `some`
    Some,
}
