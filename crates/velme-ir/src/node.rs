//! IR data types (`compiler/21` §2–4). Doc comments here are also the descriptions in the generated schema (R-IR-20).
//!
//! Names are strings, not resolved ids: an unknown input, local, record type or builtin must still parse, so the
//! validator can reject it by name with `VL0402` (R-IR-13, D-63). Likewise a `call` node parses anywhere an expression
//! may appear, so a synthesized one is rejected with `VL0402` rather than failing the schema (R-IR-02, AC-IR-02).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One IR goal: the envelope of `compiler/21` §2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    /// IR version, `MAJOR.MINOR` (R-IR-22).
    pub ir_version: String,
    /// Version of the builtin catalog the `builtin` nodes are resolved against.
    pub builtins_version: String,
    /// Name of the goal this IR implements.
    pub goal: String,
    /// Every record type reachable from the goal, by name (R-IR-01).
    pub types: BTreeMap<String, RecordType>,
    /// Goal inputs `[name, type]`, in declaration order.
    pub inputs: Vec<(String, Type)>,
    /// Declared output type.
    pub output: Type,
    /// Child goal calls, in source order; compiler-produced only, omitted in synthesized IR (R-IR-02, R-IR-09).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CallNode>,
    /// The one expression computing the output (R-IR-03).
    pub body: Node,
}

/// A record type: its fields `[name, type]` in declaration order (R-IR-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordType {
    /// Fields `[name, type]`, in declaration order.
    pub fields: Vec<(String, Type)>,
}

/// A value type (`compiler/21` §2.1).
// The scalar variants are empty struct variants because serde's `deny_unknown_fields` does not reach unit variants of
// an internally tagged enum: `{"t":"Number","x":1}` would otherwise parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", deny_unknown_fields)]
pub enum Type {
    /// Exact decimal number.
    Number {},
    /// Text.
    Text {},
    /// `true` or `false`.
    Boolean {},
    /// The type of `nothing`.
    Nothing {},
    /// `T?`: a value of `of`, or nothing.
    Optional {
        /// The present type; never itself optional, since `T??` is `T?` (§2.1).
        #[serde(with = "flat_optional")]
        #[schemars(with = "Type")]
        of: Box<Type>,
    },
    /// `List<T>`.
    List {
        /// The element type.
        of: Box<Type>,
    },
    /// A record type named in the goal's `types`.
    Record {
        /// The record type's name.
        name: String,
    },
}

/// `Optional(Optional(T))` is `Optional(T)` (§2.1), both read and written, so both spellings hash alike (D-21).
mod flat_optional {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Type;

    pub(super) fn serialize<S: Serializer>(of: &Type, serializer: S) -> Result<S::Ok, S::Error> {
        let mut of = of;
        while let Type::Optional { of: inner } = of {
            of = inner;
        }
        of.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Box<Type>, D::Error> {
        // The inner `Optional` was flattened by this same function, so one level is all that can remain.
        Ok(match Type::deserialize(deserializer)? {
            Type::Optional { of } => of,
            of => Box::new(of),
        })
    }
}

/// An expression node, tagged by `kind` (`compiler/21` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Node {
    /// A constant; `value` is JSON decoded as `type` (D-23).
    Literal {
        /// The literal's type.
        #[serde(rename = "type")]
        ty: Type,
        /// The value as JSON.
        value: serde_json::Value,
    },
    /// A goal input.
    Input {
        /// The input's name.
        name: String,
    },
    /// A call binding, `let` name or lambda parameter in scope.
    Local {
        /// The local's name.
        name: String,
    },
    /// A record value with exactly the declared fields.
    Record {
        /// The record type's name.
        #[serde(rename = "type")]
        ty: String,
        /// One expression per field, by field name.
        fields: BTreeMap<String, Node>,
    },
    /// A list value; each item is assignable to `of`.
    List {
        /// The element type.
        of: Type,
        /// The items, in order.
        items: Vec<Node>,
    },
    /// Reads `field` of a (non-optional) record.
    #[serde(rename = "field")]
    FieldGet {
        /// The record expression.
        of: Box<Node>,
        /// The field name.
        field: String,
    },
    /// A binary operator; `and`/`or` short-circuit.
    #[serde(rename = "binary")]
    BinaryOp {
        /// The operator.
        op: BinaryOperator,
        /// The left operand.
        left: Box<Node>,
        /// The right operand.
        right: Box<Node>,
    },
    /// A unary operator.
    #[serde(rename = "unary")]
    UnaryOp {
        /// The operator.
        op: UnaryOperator,
        /// The operand.
        arg: Box<Node>,
    },
    /// Sequential bindings `[name, expression]`, each visible to later ones and to `body`; no shadowing.
    Let {
        /// The bindings, in order.
        bind: Vec<(String, Node)>,
        /// The result expression.
        body: Box<Node>,
    },
    /// An expression conditional (R-IR-06).
    #[serde(rename = "if")]
    Condition {
        /// A `Boolean` condition.
        cond: Box<Node>,
        /// The value when `cond` is true.
        then: Box<Node>,
        /// The value when `cond` is false.
        #[serde(rename = "else")]
        otherwise: Box<Node>,
    },
    /// The present value of an optional `of`, else `default` (R-IR-05).
    #[serde(rename = "unwrap_or")]
    Narrow {
        /// An optional expression.
        of: Box<Node>,
        /// The value when `of` is nothing.
        default: Box<Node>,
    },
    /// Applies `fn` to every element.
    Map {
        /// The list.
        list: Box<Node>,
        /// The function.
        #[serde(rename = "fn")]
        func: Lambda,
    },
    /// Keeps the elements for which `fn` is true.
    Filter {
        /// The list.
        list: Box<Node>,
        /// The `Boolean` predicate.
        #[serde(rename = "fn")]
        func: Lambda,
    },
    /// The first element for which `fn` is true, or nothing.
    Find {
        /// The list.
        list: Box<Node>,
        /// The `Boolean` predicate.
        #[serde(rename = "fn")]
        func: Lambda,
    },
    /// Folds the list from `init`, left to right.
    Reduce {
        /// The list.
        list: Box<Node>,
        /// The initial accumulator.
        init: Box<Node>,
        /// The step function.
        #[serde(rename = "fn")]
        func: ReduceLambda,
    },
    /// Stable sort by a `Number` key (R-IR-07, D-59).
    #[serde(rename = "sort_by")]
    Sort {
        /// The list.
        list: Box<Node>,
        /// The `Number` key.
        key: Lambda,
        /// Largest key first when true.
        descending: bool,
    },
    /// True when `fn` is true for every element; true for an empty list (D-58).
    All {
        /// The list.
        list: Box<Node>,
        /// The `Boolean` predicate.
        #[serde(rename = "fn")]
        func: Lambda,
    },
    /// True when `fn` is true for some element; false for an empty list (D-58).
    Any {
        /// The list.
        list: Box<Node>,
        /// The `Boolean` predicate.
        #[serde(rename = "fn")]
        func: Lambda,
    },
    /// A call to a builtin of the goal's `builtins_version` (`language/14`).
    Builtin {
        /// The builtin's name.
        name: String,
        /// Positional arguments.
        args: Vec<Node>,
    },
    /// A child goal call; valid only in `calls` (§4, D-5).
    Call(Call),
}

/// The `fn` or `key` of a collection node; lambdas are not values (R-IR-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lambda {
    /// The element parameter.
    pub param: String,
    /// The body.
    pub body: Box<Node>,
}

/// The `fn` of a `reduce` node (R-IR-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReduceLambda {
    /// The accumulator parameter.
    pub acc: String,
    /// The element parameter.
    pub param: String,
    /// The body, giving the next accumulator.
    pub body: Box<Node>,
}

/// A child goal call (`compiler/21` §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Call {
    /// The binding name the result is stored under.
    pub binding: String,
    /// The child goal's name.
    pub goal: String,
    /// The child's signature fingerprint (R-IR-09).
    pub goal_signature: String,
    /// Positional arguments: inputs and earlier bindings only (R-IR-09).
    pub args: Vec<Node>,
}

/// An entry of a goal's `calls`: a call node, tagged `"kind": "call"` as in an expression (R-IR-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallNode {
    /// The call.
    Call(Call),
}

/// The operator of a `binary` node (`compiler/21` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOperator {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// Structural equality (D-61).
    Eq,
    /// Structural inequality (D-61).
    Ne,
    /// Short-circuit `and`.
    And,
    /// Short-circuit `or`.
    Or,
}

/// The operator of a `unary` node (`compiler/21` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOperator {
    /// Numeric negation.
    Neg,
    /// Boolean negation.
    Not,
    /// True for nothing, an empty list or empty text (D-60).
    IsEmpty,
}
