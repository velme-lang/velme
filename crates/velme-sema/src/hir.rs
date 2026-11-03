//! The typed semantic model (HIR, `compiler/20` §4): resolved ids instead of names, with spans kept (R-CMP-11).

use serde::Serialize;
use velme_builtins::{Shape, Signature, limits};
use velme_diagnostics::Span;
/// The operators and keywords HIR expressions use, so HIR's consumers need not depend on `velme-syntax`.
pub use velme_syntax::Keyword;
pub use velme_syntax::ast::{BinaryOp, Quantifier, UnaryOp};

/// A record type: an index into [`Program::types`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TypeId(pub usize);

/// A goal: an index into [`Program::goals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct GoalId(pub usize);

/// A domain type: an index into [`Program::domains`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct DomainId(pub usize);

/// A checked file (`compiler/20` §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Program {
    /// The language version from the header, or the one assumed without one (R-SYN-15).
    pub language_version: String,
    /// Record types, in source order; a record domain's underlying record is one of them (R-TYP-28).
    pub types: Vec<RecordType>,
    /// Domain types, in source order (`language/11` §7.1).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<DomainType>,
    /// Goals, in source order.
    pub goals: Vec<Goal>,
}

impl Program {
    /// The record type `id`.
    pub fn record(&self, id: TypeId) -> Option<&RecordType> {
        self.types.get(id.0)
    }

    /// The domain type `id`.
    pub fn domain(&self, id: DomainId) -> Option<&DomainType> {
        self.domains.get(id.0)
    }

    /// `ty` as a learner writes it: `List<Player?>`.
    pub fn type_name(&self, ty: &Type) -> String {
        ty.display(&self.types, &self.domains)
    }
}

/// `type Name = Base one of …`, `type Name = Base where …` or `type Name where …:` (`language/11` §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainType {
    /// Its name.
    pub name: String,
    /// `Number`, `Text`, or the underlying record of a record domain (R-TYP-28).
    pub base: Type,
    /// The closed set or the complete condition, which alone decides membership (R-CHK-15).
    pub rule: DomainRule,
    /// The simple constraints extracted from the condition's top-level conjunction (R-CHK-14).
    pub constraints: Constraints,
    /// The whole declaration.
    pub span: Span,
}

impl DomainType {
    /// Why `literal` isn't a member by the closed set or simple constraints, if it isn't: the literal as shown and the
    /// reason (`language/11` R-TYP-31). Residual conditions aren't evaluated here (R-CHK-15).
    pub fn literal_violation(&self, literal: &Literal<'_>) -> Option<(String, String)> {
        crate::domain::value_violation(self, literal)
    }
}

/// A Text or Number literal, checked against a domain's simple constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal<'a> {
    /// A Text literal.
    Text(&'a str),
    /// A Number literal, signed.
    Number(velme_builtins::Number),
}

/// What a domain admits (`language/11` R-TYP-28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainRule {
    /// `one of …`: exact Text members in declaration order.
    OneOf {
        /// The members.
        members: Vec<String>,
    },
    /// `where …`: a Boolean condition on [`ExprKind::Value`].
    Where {
        /// The condition.
        condition: Expr,
    },
}

/// The simple constraints of a domain condition (`language/13` R-CHK-14), for generation and static literal checks;
/// membership still evaluates the whole condition (R-CHK-15).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Constraints {
    /// `value > n` / `value >= n` on Number.
    pub lower: Option<Bound>,
    /// `value < n` / `value <= n` on Number.
    pub upper: Option<Bound>,
    /// `value is whole` on Number.
    pub whole: bool,
    /// `value.length > n` / `>= n` on Text.
    pub min_length: Option<Bound>,
    /// `value.length < n` / `<= n` on Text.
    pub max_length: Option<Bound>,
}

/// One end of an interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bound {
    /// The literal's exact decimal text, signed.
    pub value: String,
    /// `>=` / `<=` rather than `>` / `<`.
    pub inclusive: bool,
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
    /// A domain type, with its base (`Number`, `Text` or a record) so it can be widened without the program
    /// (R-TYP-30).
    Domain(DomainId, Box<Type>),
    /// A type that already has an error; it matches everything so the error isn't repeated (`compiler/20` R-CMP-10).
    /// Never present in a program returned by [`crate::analyze`].
    Error,
}

impl Type {
    /// `self` as a learner writes it, naming records from `types` and domains from `domains`.
    pub fn display(&self, types: &[RecordType], domains: &[DomainType]) -> String {
        match self {
            Type::Number => "Number".to_owned(),
            Type::Text => "Text".to_owned(),
            Type::Boolean => "Boolean".to_owned(),
            Type::Nothing => "Nothing".to_owned(),
            Type::Optional(inner) => format!("{}?", inner.display(types, domains)),
            Type::List(element) => format!("List<{}>", element.display(types, domains)),
            Type::Record(id) => types.get(id.0).map_or_else(|| "?".to_owned(), |r| r.name.clone()),
            Type::Domain(id, _) => domains.get(id.0).map_or_else(|| "?".to_owned(), |d| d.name.clone()),
            Type::Error => "?".to_owned(),
        }
    }

    /// `self` with every domain replaced by its base, through `List` and `?`: what reaches the IR until domains
    /// are IR types (D-158).
    pub fn erased(&self) -> Type {
        match self {
            Type::Domain(_, base) => base.erased(),
            Type::Optional(inner) => Type::Optional(Box::new(inner.erased())),
            Type::List(element) => Type::List(Box::new(element.erased())),
            other => other.clone(),
        }
    }

    /// `self` with a top-level domain replaced by its base: what operators see (R-TYP-30).
    pub fn widened(&self) -> &Type {
        match self {
            Type::Domain(_, base) => base,
            other => other,
        }
    }

    /// Whether a domain appears anywhere in `self`.
    pub fn has_domain(&self) -> bool {
        match self {
            Type::Domain(..) => true,
            Type::Optional(inner) | Type::List(inner) => inner.has_domain(),
            _ => false,
        }
    }
}

impl Goal {
    /// The goal a domain condition is walked as: no inputs, calls, checks or examples, `name` and `span` of the
    /// declaration, so a finding lands there (R-CHK-13).
    pub fn condition_holder(name: &str, output: Type, span: Span) -> Goal {
        Goal {
            name: name.to_owned(),
            params: Vec::new(),
            output,
            kind: GoalKind::Leaf,
            budget: Budget::SYSTEM,
            bindings: Vec::new(),
            plan: None,
            checks: Vec::new(),
            examples: Vec::new(),
            span,
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

/// What is synthesized for a goal (`language/12` §2, `compiler/20` §3 phase 6). An artifact manifest spells it with
/// its own `Kind` (`runtime/32` §3).
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
    /// Its type. Operands of `==`, `!=` and `is empty` keep their declared type, not a narrowed one (`language/11` §9).
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
    /// `value`, the tested value in a domain condition (`language/13` R-CHK-13).
    Value,
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
        record: TypeId,
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
    /// `x is whole` or `x is not whole` on a Number (R-CHK-13).
    IsWhole {
        /// What is tested.
        operand: Box<Expr>,
        /// `is not whole`.
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

/// The same type, where a type with an error matches anything (R-CMP-10).
fn same(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Error, _) | (_, Type::Error) => true,
        (Type::Optional(a), Type::Optional(b)) | (Type::List(a), Type::List(b)) => same(a, b),
        _ => a == b,
    }
}

/// R-TYP-20, R-TYP-30: `from` is assignable to `to` iff it [widens] to `to`, or `to` is `U?` and `from` is
/// `Nothing` or widens to `U`. A base never becomes a domain here; source literals reach domains through the
/// expected-type pass (R-TYP-31).
pub fn assignable(from: &Type, to: &Type) -> bool {
    widens(from, to) || matches!(to, Type::Optional(inner) if *from == Type::Nothing || widens(from, inner))
}

/// `from` is `to`, or `to` with some of `from`'s domains widened to their bases, also under matching `List` and `?`
/// (R-TYP-30); lists stay otherwise invariant (R-TYP-13). A different domain never widens to `D`.
pub fn widens(from: &Type, to: &Type) -> bool {
    match (from, to) {
        (Type::Error, _) | (_, Type::Error) => true,
        (Type::Domain(a, _), Type::Domain(b, _)) => a == b,
        (Type::Domain(_, base), _) => widens(base, to),
        (Type::Optional(a), Type::Optional(b)) | (Type::List(a), Type::List(b)) => widens(a, b),
        _ => from == to,
    }
}

/// D-163's `compat`: [`assignable`], and also a value with no domain at the top filling a domain position of its
/// base, recursively under `List` and `?`. Optional values enter `?` only at the top, so `List<Number>` still doesn't
/// fit `List<Number?>` (R-TYP-13). Source uses it for literals, record fields and list items (R-TYP-31).
pub fn compat(from: &Type, to: &Type) -> bool {
    compat_at(from, to, true)
}

fn compat_at(from: &Type, to: &Type, top: bool) -> bool {
    match (from, to) {
        (Type::Error, _) | (_, Type::Error) => true,
        (Type::Domain(a, _), Type::Domain(b, _)) => a == b,
        (_, Type::Domain(_, base)) => compat_at(from, base, top),
        (Type::List(a), Type::List(b)) | (Type::Optional(a), Type::Optional(b)) => compat_at(a, b, false),
        // Before a source domain is widened, so a different domain never fits `D?` (R-TYP-30).
        (_, Type::Optional(inner)) if top => *from == Type::Nothing || compat_at(from, inner, true),
        (Type::Domain(_, base), _) => compat_at(base, to, top),
        _ => from == to,
    }
}

/// R-TYP-30, D-163: `found` is a direct expression of a different domain where `expected` is needed — it doesn't fit
/// `expected` ([`compat`]) though it would with every domain erased. Sema reports it as `VL0204`, the IR validator as
/// `VL0402` (R-IR-25); both call this so they can't drift.
pub fn domain_mismatch(found: &Type, expected: &Type) -> bool {
    !compat(found, expected) && compat(&found.erased(), &expected.erased())
}

/// R-TYP-26, R-TYP-30: the least type both `a` and `b` are assignable to, if there is one. Distinct domains, or a
/// domain and its base, join at the base.
pub fn join(a: &Type, b: &Type) -> Option<Type> {
    if assignable(a, b) {
        Some(b.clone())
    } else if assignable(b, a) {
        Some(a.clone())
    } else if *a == Type::Nothing {
        Some(Type::Optional(Box::new(b.clone())))
    } else if *b == Type::Nothing {
        Some(Type::Optional(Box::new(a.clone())))
    } else if a.has_domain() || b.has_domain() {
        join(&a.erased(), &b.erased())
    } else {
        None
    }
}

/// R-TYP-30: unifying distinct domains, or a domain and its base, yields the base, recursively through containers:
/// `b` if `a` widens to it, `a` if `b` does, else both erased when they erase alike. `None` when no domain is involved.
fn domain_join(a: &Type, b: &Type) -> Option<Type> {
    if !(a.has_domain() || b.has_domain()) || a.erased() != b.erased() {
        None
    } else if widens(a, b) {
        Some(b.clone())
    } else if widens(b, a) {
        Some(a.clone())
    } else {
        Some(a.erased())
    }
}

/// The output type of a call to a built-in with `signature` on arguments of types `args`, or `None` if they don't fit
/// it (`language/14`): shared by checks and the IR validator so both read the catalog alike (R-BLT-01).
pub fn signature_output(signature: &Signature, args: &[Type]) -> Option<Type> {
    let mut vars = Vars::default();
    (signature.params.len() == args.len() && signature.params.iter().zip(args).all(|(s, a)| vars.unify(s, a)))
        .then(|| vars.instantiate(&signature.output).unwrap_or(Type::Error))
}

/// What `T` and `U` stand for in one call of a built-in.
#[derive(Default)]
pub(crate) struct Vars {
    t: Option<Type>,
    u: Option<Type>,
}

impl Vars {
    fn slot(&mut self, shape: &Shape) -> Option<&mut Option<Type>> {
        match shape {
            Shape::T => Some(&mut self.t),
            Shape::U => Some(&mut self.u),
            _ => None,
        }
    }

    /// `shape` as a type, if everything in it is known.
    pub(crate) fn instantiate(&self, shape: &Shape) -> Option<Type> {
        Some(match shape {
            Shape::Number => Type::Number,
            Shape::Text => Type::Text,
            Shape::Boolean => Type::Boolean,
            Shape::T => self.t.clone()?,
            Shape::U => self.u.clone()?,
            Shape::List(element) => Type::List(Box::new(self.instantiate(element)?)),
            Shape::Optional(inner) => Type::Optional(Box::new(self.instantiate(inner)?)),
            Shape::Lambda(..) => return None,
        })
    }

    /// Whether an argument of type `ty` fits `shape`, fixing `T` and `U` the first time they're seen.
    pub(crate) fn unify(&mut self, shape: &Shape, ty: &Type) -> bool {
        match (shape, ty) {
            (_, Type::Error) => true,
            (Shape::T | Shape::U, _) => match self.slot(shape) {
                Some(Some(bound)) if assignable(ty, bound) => true,
                Some(Some(bound)) => domain_join(ty, bound).map(|joined| *bound = joined).is_some(),
                Some(slot) => {
                    *slot = Some(ty.clone());
                    true
                }
                None => false,
            },
            (Shape::Optional(_), Type::Nothing) => true,
            _ => self.exact(shape, ty),
        }
    }

    /// Whether `ty` is exactly `shape`: inside a list, types are invariant (R-TYP-13).
    fn exact(&mut self, shape: &Shape, ty: &Type) -> bool {
        match (shape, ty) {
            // A fixed-shape parameter takes a domain as its base; `T` and `U` keep the domain (R-TYP-30).
            (Shape::Number | Shape::Text | Shape::Boolean | Shape::List(_), Type::Domain(_, base)) => {
                self.exact(shape, base)
            }
            (_, Type::Error)
            | (Shape::Number, Type::Number)
            | (Shape::Text, Type::Text)
            | (Shape::Boolean, Type::Boolean) => true,
            (Shape::T | Shape::U, _) => match self.slot(shape) {
                Some(Some(bound)) if same(ty, bound) => true,
                Some(Some(bound)) => domain_join(ty, bound).map(|joined| *bound = joined).is_some(),
                Some(slot) => {
                    *slot = Some(ty.clone());
                    true
                }
                None => false,
            },
            (Shape::List(s), Type::List(t)) | (Shape::Optional(s), Type::Optional(t)) => self.exact(s, t),
            _ => false,
        }
    }

    /// `shape` for a learner: its type if known, else what kind of value it is.
    pub(crate) fn describe(&self, shape: &Shape, types: &[RecordType], domains: &[DomainType]) -> String {
        if let Some(ty) = self.instantiate(shape) {
            return ty.display(types, domains);
        }
        match shape {
            Shape::List(_) => "a list".to_owned(),
            Shape::Optional(_) => "a value that may be `nothing`".to_owned(),
            _ => "a value".to_owned(),
        }
    }
}
