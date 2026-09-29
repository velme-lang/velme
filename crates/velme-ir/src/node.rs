//! IR data types (`compiler/21` §2–4). Doc comments here are also the descriptions in the generated schema (R-IR-20).
//!
//! Names are strings, not resolved ids: an unknown input, local, record type or builtin must still parse, so the
//! validator can reject it by name with `VL0402` (R-IR-13, D-63). Likewise a `call` node parses anywhere an expression
//! may appear, so a synthesized one is rejected with `VL0402` rather than failing the schema (R-IR-02, AC-IR-02).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
        #[schemars(with = "serde_json::Value")]
        value: LiteralValue,
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
        /// Whether the result's element type is optional, as the validator typed it; not part of the document.
        #[serde(skip)]
        #[schemars(skip)]
        items: ItemShape,
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

impl Node {
    /// This node and every node under it in pre-order: each node, then its children in field order, a `record`'s
    /// fields by name and a collection node's lambda body after its list (and `init`).
    pub fn preorder(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            out.push(node);
            let children: Vec<&Node> = match node {
                Node::Literal { .. } | Node::Input { .. } | Node::Local { .. } => Vec::new(),
                Node::Record { fields, .. } => fields.values().collect(),
                Node::List { items, .. } | Node::Builtin { args: items, .. } => items.iter().collect(),
                Node::FieldGet { of, .. } => vec![of],
                Node::BinaryOp { left, right, .. } => vec![left, right],
                Node::UnaryOp { arg, .. } => vec![arg],
                Node::Let { bind, body } => bind.iter().map(|(_, v)| v).chain([&**body]).collect(),
                Node::Condition { cond, then, otherwise } => vec![cond, then, otherwise],
                Node::Narrow { of, default } => vec![of, default],
                Node::Map { list, func, .. }
                | Node::Filter { list, func }
                | Node::Find { list, func }
                | Node::All { list, func }
                | Node::Any { list, func }
                | Node::Sort { list, key: func, .. } => vec![list, &func.body],
                Node::Reduce { list, init, func } => vec![list, init, &func.body],
                Node::Call(call) => call.args.iter().collect(),
            };
            stack.extend(children.into_iter().rev());
        }
        out
    }

    /// The lambda body of a collection node, which runs once per element.
    pub fn lambda_body(&self) -> Option<&Node> {
        match self {
            Node::Map { func, .. }
            | Node::Filter { func, .. }
            | Node::Find { func, .. }
            | Node::All { func, .. }
            | Node::Any { func, .. }
            | Node::Sort { key: func, .. } => Some(&func.body),
            Node::Reduce { func, .. } => Some(&func.body),
            _ => None,
        }
    }

    /// The node's `kind` tag, for diagnostics; `kind_matches_the_serialized_tag` keeps it equal to the serde spelling.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Node::Literal { .. } => "literal",
            Node::Input { .. } => "input",
            Node::Local { .. } => "local",
            Node::Record { .. } => "record",
            Node::List { .. } => "list",
            Node::FieldGet { .. } => "field",
            Node::BinaryOp { .. } => "binary",
            Node::UnaryOp { .. } => "unary",
            Node::Let { .. } => "let",
            Node::Condition { .. } => "if",
            Node::Narrow { .. } => "unwrap_or",
            Node::Map { .. } => "map",
            Node::Filter { .. } => "filter",
            Node::Find { .. } => "find",
            Node::Reduce { .. } => "reduce",
            Node::Sort { .. } => "sort_by",
            Node::All { .. } => "all",
            Node::Any { .. } => "any",
            Node::Builtin { .. } => "builtin",
            Node::Call(_) => "call",
        }
    }
}

/// A `literal` node's value: its JSON and, once the validator has decoded it (`compiler/21` §6 stage 4), the value it
/// stands for, which a back end reads instead of decoding the JSON on every evaluation (D-83). Only the JSON is read,
/// written and compared, so a decoded literal equals one never validated.
#[derive(Debug, Clone, Default)]
pub struct LiteralValue {
    json: serde_json::Value,
    decoded: OnceLock<velme_builtins::Value>,
}

impl LiteralValue {
    /// The value as JSON.
    pub fn json(&self) -> &serde_json::Value {
        &self.json
    }

    /// The value the validator decoded it to; `None` for a literal that hasn't passed the validator.
    pub fn decoded(&self) -> Option<&velme_builtins::Value> {
        self.decoded.get()
    }

    /// Records what the validator decoded the JSON to; a literal is decoded once, so a second value is ignored.
    pub(crate) fn set_decoded(&self, value: velme_builtins::Value) {
        let _ = self.decoded.set(value);
    }
}

impl From<serde_json::Value> for LiteralValue {
    fn from(json: serde_json::Value) -> Self {
        LiteralValue {
            json,
            decoded: OnceLock::new(),
        }
    }
}

impl PartialEq for LiteralValue {
    fn eq(&self, other: &Self) -> bool {
        self.json == other.json
    }
}

impl Eq for LiteralValue {}

impl Serialize for LiteralValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.json.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LiteralValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(deserializer).map(LiteralValue::from)
    }
}

/// Whether the item type of the list a node makes is optional, as the validator typed it (`compiler/21` §6): what a
/// back end needs to size the list (`runtime/30` §7.1) and the one thing about a `map` result it can't see in the
/// document. It is set once, when the node is validated, is no part of the document, and doesn't count in comparisons.
#[derive(Debug, Default)]
pub struct ItemShape(OnceLock<bool>);

impl ItemShape {
    /// Whether the items are optional; `None` for a node the validator hasn't typed.
    pub fn optional(&self) -> Option<bool> {
        self.0.get().copied()
    }

    /// Records the validator's typing; a node is typed once, so a second value is ignored.
    pub(crate) fn set(&self, optional: bool) {
        let _ = self.0.set(optional);
    }
}

impl Clone for ItemShape {
    fn clone(&self) -> Self {
        let shape = ItemShape::default();
        if let Some(optional) = self.0.get() {
            shape.set(*optional);
        }
        shape
    }
}

impl PartialEq for ItemShape {
    fn eq(&self, _: &ItemShape) -> bool {
        true
    }
}

impl Eq for ItemShape {}

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

#[cfg(test)]
mod tests {
    use super::Node;

    #[test]
    fn kind_matches_the_serialized_tag() {
        let input = r#"{"kind": "input", "name": "x"}"#;
        let lambda = format!(r#"{{"param": "p", "body": {input}}}"#);
        let nodes = [
            r#"{"kind": "literal", "type": {"t": "Nothing"}, "value": null}"#.to_owned(),
            input.to_owned(),
            r#"{"kind": "local", "name": "x"}"#.to_owned(),
            r#"{"kind": "record", "type": "R", "fields": {}}"#.to_owned(),
            r#"{"kind": "list", "of": {"t": "Number"}, "items": []}"#.to_owned(),
            format!(r#"{{"kind": "field", "of": {input}, "field": "f"}}"#),
            format!(r#"{{"kind": "binary", "op": "add", "left": {input}, "right": {input}}}"#),
            format!(r#"{{"kind": "unary", "op": "neg", "arg": {input}}}"#),
            format!(r#"{{"kind": "let", "bind": [], "body": {input}}}"#),
            format!(r#"{{"kind": "if", "cond": {input}, "then": {input}, "else": {input}}}"#),
            format!(r#"{{"kind": "unwrap_or", "of": {input}, "default": {input}}}"#),
            format!(r#"{{"kind": "map", "list": {input}, "fn": {lambda}}}"#),
            format!(r#"{{"kind": "filter", "list": {input}, "fn": {lambda}}}"#),
            format!(r#"{{"kind": "find", "list": {input}, "fn": {lambda}}}"#),
            format!(
                r#"{{"kind": "reduce", "list": {input}, "init": {input}, "fn": {{"acc": "a", "param": "p", "body": {input}}}}}"#
            ),
            format!(r#"{{"kind": "sort_by", "list": {input}, "key": {lambda}, "descending": false}}"#),
            format!(r#"{{"kind": "all", "list": {input}, "fn": {lambda}}}"#),
            format!(r#"{{"kind": "any", "list": {input}, "fn": {lambda}}}"#),
            r#"{"kind": "builtin", "name": "sum", "args": []}"#.to_owned(),
            r#"{"kind": "call", "binding": "b", "goal": "G", "goal_signature": "b3:00", "args": []}"#.to_owned(),
        ];
        let mut kinds = std::collections::BTreeSet::new();
        for json in &nodes {
            let node: Node = crate::from_json_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
            let tag = serde_json::to_value(&node)
                .ok()
                .and_then(|v| v["kind"].as_str().map(str::to_owned));
            assert_eq!(tag.as_deref(), Some(node.kind()), "{json}");
            kinds.insert(node.kind());
        }
        assert_eq!(kinds.len(), 20, "every variant is covered");
    }
}
