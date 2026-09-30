//! The typed valid-IR generator (`runtime/31` R-SBX-15, `delivery/51` §2, D-118): from a fuzzer's or a property
//! test's bytes, a leaf goal, its IR and inputs for it, well typed and within the validator's limits (`compiler/21`
//! §7), using every node kind, operator and value built-in there is. [`check`] runs it on both backends, also within
//! limits cut short, so the failures are compared as well as the values. The same bytes give the same goal.

use std::collections::BTreeMap;
use std::sync::Arc;

use arbitrary::Unstructured;
use serde_json::Value as Json;
use velme_builtins::BUILTINS_VERSION;
use velme_builtins::execution::{Limits, Spent};
use velme_diagnostics::Code;
use velme_ir::limits::{MAX_COLLECTION_NESTING, MAX_LIST_ITEMS};
use velme_ir::{
    BinaryOperator, Goal, IR_VERSION, ItemShape, Lambda, LiteralValue, Node, RecordType, ReduceLambda, Type,
    UnaryOperator, decode_str,
};
use velme_runtime::Wasm;

use crate::differential::{Leaf, compare, differential};
use crate::{goal_id, program, valid_ir};

/// The generated goal's name.
const GOAL: &str = "G";

/// How deep an expression grows before its branches end in an input, a local or a literal.
const DEPTH: usize = 8;

/// How many expressions a body grows before the rest end like that.
const BUDGET: usize = 96;

/// Numbers a literal or an input may be: ties to round, values only an exact decimal gets right, and the spellings
/// JSON allows (D-23, D-36).
const NUMBERS: [&str; 18] = [
    "0", "1", "-1", "2", "3", "7", "100", "0.1", "0.2", "0.3", "0.5", "-0.5", "2.4", "2.5", "-2.5", "2.50", "-0", "1e2",
];

/// Numbers at the edges, taken less often since most arithmetic on them fails: of the `Number` range, of its scale,
/// and of the 64-bit integers `random` takes (R-TYP-07).
const EDGES: [&str; 8] = [
    "123.456",
    "1000000",
    "0.0000000000000000000000000001",
    "3.3333333333333333333333333333",
    "79228162514264337593543950335",
    "-79228162514264337593543950335",
    "9223372036854775807",
    "-9223372036854775808",
];

/// Texts a literal or an input may be: empty, multi-byte, escaped, and one long enough to be shown cut short.
const TEXTS: [&str; 10] = [
    "",
    "a",
    "abc",
    "héllo",
    "日本語",
    "🙂",
    "a\"b\\c",
    "line\nbreak\t\u{0}",
    "Lina",
    "the quick brown fox jumps over the lazy dog, and then the quick brown fox jumps over the lazy dog again",
];

/// The `range` counts past a small one: the most a list may hold, one more, and more than a `Number` makes a list of.
const RANGES: [&str; 3] = ["10000", "10001", "1.5"];

/// A generated leaf goal: the program declaring it, its IR, inputs to run it on, and where to cut its limits short.
#[derive(Debug, Clone)]
pub struct Generated {
    /// The Velme source of its record types and signature.
    pub source: String,
    /// Its IR document.
    pub document: String,
    /// Each case's inputs as JSON, one per input in declared order.
    pub inputs: Vec<Vec<String>>,
    /// Eighths of the fuel, then of the memory, a case spent within the system limits, each run within again.
    pub cuts: (u64, u64),
}

impl Generated {
    /// The goal checked and its IR validated, with each case's inputs decoded.
    pub fn leaf(&self) -> Leaf {
        let program = program(&self.source);
        let goal = goal_id(&program, GOAL);
        let ir = valid_ir(&program, &self.document);
        let params = &program.goals[goal.0].params;
        let cases = self
            .inputs
            .iter()
            .map(|case| {
                case.iter()
                    .zip(params)
                    .map(|(text, param)| {
                        decode_str(text, &param.ty, &program).unwrap_or_else(|e| panic!("{text} doesn't decode: {e:?}"))
                    })
                    .collect()
            })
            .collect();
        Leaf {
            name: GOAL.to_owned(),
            program,
            goal,
            ir,
            cases,
        }
    }

    /// The limits cut short from what a case spent within the system's, a fraction of its fuel and then of its
    /// memory, each with the failure it must stop the run with when it is strictly below the figure spent.
    fn cut(&self, spent: Spent) -> impl Iterator<Item = (Limits, Option<Code>)> {
        let fuel = spent.fuel.saturating_mul(self.cuts.0) / 8;
        let memory = spent.memory.saturating_mul(self.cuts.1) / 8;
        [
            (
                Limits { fuel, ..Limits::SYSTEM },
                (fuel < spent.fuel).then_some(Code::BudgetExceeded),
            ),
            (
                Limits {
                    memory,
                    ..Limits::SYSTEM
                },
                (memory < spent.memory).then_some(Code::MemoryLimitExceeded),
            ),
        ]
        .into_iter()
        .filter(|(limits, _)| limits.fuel > 0 && limits.memory > 0)
    }
}

/// `generated` on both backends, case by case: [`differential`], then [`compare`] within each of its cut limits, each
/// cut below what the case spent failing with its limit's code. What differs, with the goal and the inputs, if
/// anything does.
pub fn check(wasm: &Arc<Wasm>, generated: &Generated) -> Result<(), String> {
    let leaf = generated.leaf();
    for inputs in &leaf.cases {
        let shown = |e: String| {
            let document = &generated.document;
            format!(
                "{e}\n  source: {}\n  IR: {document}\n  inputs: {inputs:?}",
                generated.source
            )
        };
        let (_, spent) = differential(wasm, &leaf.program, leaf.goal, &leaf.ir, inputs).map_err(shown)?;
        for (limits, code) in generated.cut(spent) {
            let (result, _) = compare(wasm, &leaf.program, leaf.goal, &leaf.ir, inputs, limits)
                .map_err(|e| shown(format!("within {limits:?}: {e}")))?;
            match (code, result) {
                (None, _) => {}
                (Some(code), Err(diagnostic)) if diagnostic.code == code => {}
                (Some(code), other) => {
                    return Err(shown(format!("within {limits:?}: expected {code:?}, got {other:?}")));
                }
            }
        }
    }
    Ok(())
}

/// A leaf goal made from `u`'s bytes; fewer bytes make a smaller one, and none a literal of a scalar.
pub fn generate(u: &mut Unstructured<'_>) -> Generated {
    let mut g = Generator {
        u,
        records: Vec::new(),
        usable: Vec::new(),
        inputs: Vec::new(),
        scope: Vec::new(),
        fresh: 0,
        depth: 0,
        nesting: 0,
        budget: BUDGET,
    };
    for index in 0..g.pick(4) {
        let earlier: Vec<usize> = (0..index).collect();
        let fields = (0..=g.pick(3)).map(|f| (format!("f{f}"), g.ty(2, &earlier))).collect();
        g.records.push(fields);
    }
    let all: Vec<usize> = (0..g.records.len()).collect();
    for i in 0..g.pick(4) {
        let ty = g.ty(2, &all);
        g.inputs.push((format!("i{i}"), ty));
    }
    let output = g.ty(2, &all);
    g.usable = g.reached(&output);
    let body = g.expr(&output);
    let cases = (0..=g.pick(2))
        .map(|_| {
            let inputs = g.inputs.clone();
            inputs.iter().map(|(_, ty)| g.json(ty, 6).to_string()).collect()
        })
        .collect();
    let cuts = (1 + g.pick(7) as u64, 1 + g.pick(7) as u64);
    Generated {
        source: g.source(&output),
        document: g.document(&output, body),
        inputs: cases,
        cuts,
    }
}

/// A value type of the generator: `Nothing` only ever appears as the literal `nothing`, which the emitter takes only
/// where a `T?` is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    Number,
    Text,
    Boolean,
    Optional(Box<Ty>),
    List(Box<Ty>),
    /// The record type `R<n>`.
    Record(usize),
}

impl Ty {
    /// `T?`, where `T??` is `T?`.
    fn optional(self) -> Ty {
        match self {
            Ty::Optional(_) => self,
            other => Ty::Optional(Box::new(other)),
        }
    }

    fn list(self) -> Ty {
        Ty::List(Box::new(self))
    }

    fn ir(&self) -> Type {
        match self {
            Ty::Number => Type::Number {},
            Ty::Text => Type::Text {},
            Ty::Boolean => Type::Boolean {},
            Ty::Optional(of) => Type::Optional { of: Box::new(of.ir()) },
            Ty::List(of) => Type::List { of: Box::new(of.ir()) },
            Ty::Record(r) => Type::Record { name: format!("R{r}") },
        }
    }

    fn spelled(&self) -> String {
        match self {
            Ty::Number => "Number".to_owned(),
            Ty::Text => "Text".to_owned(),
            Ty::Boolean => "Boolean".to_owned(),
            Ty::Optional(of) => format!("{}?", of.spelled()),
            Ty::List(of) => format!("List<{}>", of.spelled()),
            Ty::Record(r) => format!("R{r}"),
        }
    }
}

/// One way to grow an expression of a wanted type.
enum Move {
    Leaf,
    If,
    Let,
    Unwrap,
    Field(usize, String),
    Reduce,
    Binary(BinaryOperator),
    Equal(BinaryOperator),
    Unary(UnaryOperator),
    Builtin(&'static str),
    All,
    Any,
    List,
    Map,
    Filter,
    Sort,
    Record,
    Find,
    OrNothing,
    Deep,
}

struct Generator<'u, 'd> {
    u: &'u mut Unstructured<'d>,
    /// Each record type's fields; `R<n>` refers only to records before it.
    records: Vec<Vec<(String, Ty)>>,
    /// The records the goal's signature reaches, which alone its body may use (R-IR-01).
    usable: Vec<usize>,
    inputs: Vec<(String, Ty)>,
    /// The locals in scope; every name is fresh, so none shadows another (R-IR-12).
    scope: Vec<(String, Ty)>,
    fresh: usize,
    depth: usize,
    /// Collection nodes around the current expression's lambda (R-IR-17, D-79).
    nesting: usize,
    budget: usize,
}

impl Generator<'_, '_> {
    /// A choice among `n`: `0` once the bytes run out, so a generator short of bytes ends every branch.
    fn pick(&mut self, n: usize) -> usize {
        match n.checked_sub(1) {
            None | Some(0) => 0,
            Some(last) => self.u.int_in_range(0..=last).unwrap_or(0),
        }
    }

    /// True one time in `n`, and never once the bytes run out.
    fn rarely(&mut self, n: usize) -> bool {
        n > 1 && self.pick(n) == n - 1
    }

    fn coin(&mut self) -> bool {
        self.pick(2) == 1
    }

    /// A type nesting at most `depth` lists and optionals, whose records are among `records`.
    fn ty(&mut self, depth: usize, records: &[usize]) -> Ty {
        let composite = if depth > 0 { 2 } else { 0 };
        let record = usize::from(!records.is_empty());
        let k = self.pick(3 + composite + record);
        match k {
            0 => Ty::Number,
            1 => Ty::Text,
            2 => Ty::Boolean,
            3 | 4 if composite > 0 => {
                let inner = self.ty(depth - 1, records);
                if k == 3 { inner.optional() } else { inner.list() }
            }
            _ => {
                let at = self.pick(records.len());
                records.get(at).map_or(Ty::Number, |r| Ty::Record(*r))
            }
        }
    }

    /// A type for a local, a lambda's element or an operand: its records among those the body may use.
    fn local_ty(&mut self) -> Ty {
        let usable = self.usable.clone();
        self.ty(2, &usable)
    }

    /// The records the inputs and `output` reach, through fields too.
    fn reached(&self, output: &Ty) -> Vec<usize> {
        fn walk(ty: &Ty, records: &[Vec<(String, Ty)>], out: &mut Vec<usize>) {
            match ty {
                Ty::Optional(of) | Ty::List(of) => walk(of, records, out),
                Ty::Record(r) if !out.contains(r) => {
                    out.push(*r);
                    for (_, field) in &records[*r] {
                        walk(field, records, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for ty in self.inputs.iter().map(|(_, ty)| ty).chain([output]) {
            walk(ty, &self.records, &mut out);
        }
        out.sort_unstable();
        out
    }

    fn name(&mut self) -> String {
        self.fresh += 1;
        format!("v{}", self.fresh)
    }

    fn number(&mut self) -> Json {
        let text = if self.rarely(6) {
            EDGES[self.pick(EDGES.len())]
        } else if self.coin() {
            return Json::from(self.small(-20, 20));
        } else {
            NUMBERS[self.pick(NUMBERS.len())]
        };
        serde_json::from_str(text).expect("each of NUMBERS and EDGES is JSON")
    }

    /// A whole number from `low` to `high`.
    fn small(&mut self, low: i64, high: i64) -> i64 {
        let span = usize::try_from(high - low).unwrap_or(0);
        low + i64::try_from(self.pick(span + 1)).unwrap_or(0)
    }

    /// The literal `n`.
    fn whole(n: i64) -> Node {
        Node::Literal {
            ty: Type::Number {},
            value: LiteralValue::from(Json::from(n)),
        }
    }

    /// A JSON value of `ty`, whose lists have at most `items` items.
    fn json(&mut self, ty: &Ty, items: usize) -> Json {
        match ty {
            Ty::Number => self.number(),
            Ty::Text => Json::from(TEXTS[self.pick(TEXTS.len())]),
            Ty::Boolean => Json::from(self.coin()),
            Ty::Optional(of) => {
                if self.pick(3) == 0 {
                    Json::Null
                } else {
                    self.json(of, items)
                }
            }
            Ty::List(of) => (0..self.pick(items + 1)).map(|_| self.json(of, items)).collect(),
            Ty::Record(r) => {
                let fields = self.records[*r].clone();
                let object = fields.iter().map(|(name, ty)| (name.clone(), self.json(ty, items)));
                Json::Object(object.collect())
            }
        }
    }

    fn literal(&mut self, ty: &Ty) -> Node {
        let value = self.json(ty, 3);
        Node::Literal {
            ty: ty.ir(),
            value: LiteralValue::from(value),
        }
    }

    /// The literal `nothing`, of type `Nothing`.
    fn nothing() -> Node {
        Node::Literal {
            ty: Type::Nothing {},
            value: LiteralValue::from(Json::Null),
        }
    }

    /// An input, a local or a literal of `ty`.
    fn leaf(&mut self, ty: &Ty) -> Node {
        let inputs = self
            .inputs
            .iter()
            .filter(|(_, t)| t == ty)
            .map(|(n, _)| Node::Input { name: n.clone() });
        let locals = self
            .scope
            .iter()
            .filter(|(_, t)| t == ty)
            .map(|(n, _)| Node::Local { name: n.clone() });
        let mut named: Vec<Node> = inputs.chain(locals).collect();
        match self.pick(named.len() + 1) {
            0 => self.literal(ty),
            k => named.swap_remove(k - 1),
        }
    }

    /// An expression whose type is exactly `ty`.
    fn expr(&mut self, ty: &Ty) -> Node {
        self.budget = self.budget.saturating_sub(1);
        if self.depth >= DEPTH || self.budget == 0 {
            return self.leaf(ty);
        }
        self.depth += 1;
        let node = self.grow(ty);
        self.depth -= 1;
        node
    }

    /// A body run inside a collection node's function, with `params` bound.
    fn inside(&mut self, params: Vec<(String, Ty)>, ty: &Ty) -> Node {
        let mark = self.scope.len();
        self.scope.extend(params);
        self.nesting += 1;
        let body = self.expr(ty);
        self.nesting -= 1;
        self.scope.truncate(mark);
        body
    }

    fn lambda(&mut self, element: Ty, ty: &Ty) -> Lambda {
        let param = self.name();
        let body = self.inside(vec![(param.clone(), element)], ty);
        Lambda {
            param,
            body: Box::new(body),
        }
    }

    fn moves(&mut self, ty: &Ty) -> Vec<Move> {
        let mut moves = vec![Move::Leaf, Move::If, Move::Let];
        let collections = self.nesting < MAX_COLLECTION_NESTING;
        if collections {
            moves.push(Move::Reduce);
        }
        if !matches!(ty, Ty::Optional(_)) {
            moves.push(Move::Unwrap);
        }
        for r in self.usable.clone() {
            for (name, field) in &self.records[r] {
                if field == ty {
                    moves.push(Move::Field(r, name.clone()));
                }
            }
        }
        match ty {
            Ty::Number => {
                use BinaryOperator::{Add, Div, Mul, Sub};
                moves.extend([Add, Sub, Mul, Div].map(Move::Binary));
                moves.push(Move::Unary(UnaryOperator::Neg));
                let builtins = ["length", "sum", "abs", "floor", "ceil", "round", "clamp", "random"];
                moves.extend(builtins.map(Move::Builtin));
                moves.push(Move::Deep);
            }
            Ty::Boolean => {
                use BinaryOperator::{And, Eq, Ge, Gt, Le, Lt, Ne, Or};
                moves.extend([Lt, Le, Gt, Ge, And, Or].map(Move::Binary));
                moves.extend([Eq, Ne].map(Move::Equal));
                moves.extend([UnaryOperator::Not, UnaryOperator::IsEmpty].map(Move::Unary));
                moves.extend(["is_empty", "contains"].map(Move::Builtin));
                if collections {
                    moves.extend([Move::All, Move::Any]);
                }
            }
            Ty::Text => moves.extend(["concat", "to_text"].map(Move::Builtin)),
            Ty::List(of) => {
                moves.push(Move::List);
                if collections {
                    moves.extend([Move::Map, Move::Filter, Move::Sort]);
                }
                if **of == Ty::Number {
                    moves.push(Move::Builtin("range"));
                }
            }
            Ty::Record(_) => moves.push(Move::Record),
            Ty::Optional(of) => {
                moves.push(Move::OrNothing);
                if collections {
                    moves.push(Move::Find);
                }
                if **of == Ty::Number {
                    moves.extend(["maximum", "minimum"].map(Move::Builtin));
                }
            }
        }
        moves
    }

    fn grow(&mut self, ty: &Ty) -> Node {
        let mut moves = self.moves(ty);
        let at = self.pick(moves.len());
        match moves.swap_remove(at) {
            Move::Leaf => self.leaf(ty),
            Move::If => Node::Condition {
                cond: Box::new(self.expr(&Ty::Boolean)),
                then: Box::new(self.expr(ty)),
                otherwise: Box::new(self.expr(ty)),
            },
            Move::Let => {
                let mark = self.scope.len();
                let mut bind = Vec::new();
                for _ in 0..=self.pick(3) {
                    let local = self.local_ty();
                    let value = self.expr(&local);
                    let name = self.name();
                    self.scope.push((name.clone(), local));
                    bind.push((name, value));
                }
                let body = Box::new(self.expr(ty));
                self.scope.truncate(mark);
                Node::Let { bind, body }
            }
            Move::Unwrap => {
                let of = if self.rarely(8) {
                    Self::nothing()
                } else {
                    self.expr(&ty.clone().optional())
                };
                Node::Narrow {
                    of: Box::new(of),
                    default: Box::new(self.expr(ty)),
                }
            }
            Move::Field(r, field) => Node::FieldGet {
                of: Box::new(self.expr(&Ty::Record(r))),
                field,
            },
            Move::Reduce => {
                let element = self.local_ty();
                let list = Box::new(self.expr(&element.clone().list()));
                self.nesting += 1;
                let init = Box::new(self.expr(ty));
                self.nesting -= 1;
                let (acc, param) = (self.name(), self.name());
                let params = vec![(acc.clone(), ty.clone()), (param.clone(), element)];
                let body = Box::new(self.inside(params, ty));
                Node::Reduce {
                    list,
                    init,
                    func: ReduceLambda { acc, param, body },
                }
            }
            Move::Binary(op) => {
                let operand = match op {
                    BinaryOperator::And | BinaryOperator::Or => Ty::Boolean,
                    _ => Ty::Number,
                };
                Node::BinaryOp {
                    op,
                    left: Box::new(self.expr(&operand)),
                    right: Box::new(self.expr(&operand)),
                }
            }
            Move::Equal(op) => {
                let operand = self.local_ty();
                let left = Box::new(self.expr(&operand));
                let right = if matches!(operand, Ty::Optional(_)) && self.rarely(4) {
                    Self::nothing()
                } else {
                    self.expr(&operand)
                };
                Node::BinaryOp {
                    op,
                    left,
                    right: Box::new(right),
                }
            }
            Move::Unary(op) => {
                let arg = match op {
                    UnaryOperator::Neg => self.expr(&Ty::Number),
                    UnaryOperator::Not => self.expr(&Ty::Boolean),
                    UnaryOperator::IsEmpty if self.rarely(8) => Self::nothing(),
                    UnaryOperator::IsEmpty => {
                        let arg = self.emptiable();
                        self.expr(&arg)
                    }
                };
                Node::UnaryOp { op, arg: Box::new(arg) }
            }
            Move::Builtin(name) => {
                let args = self.args(name);
                Node::Builtin {
                    name: name.to_owned(),
                    args,
                }
            }
            Move::All => {
                let (list, func) = self.predicate();
                Node::All { list, func }
            }
            Move::Any => {
                let (list, func) = self.predicate();
                Node::Any { list, func }
            }
            Move::List => {
                let Ty::List(of) = ty else { return self.leaf(ty) };
                let items = if self.rarely(64) {
                    (0..MAX_LIST_ITEMS).map(|_| self.literal(of)).collect()
                } else {
                    (0..self.pick(5)).map(|_| self.expr(of)).collect()
                };
                Node::List { of: of.ir(), items }
            }
            Move::Map => {
                let Ty::List(of) = ty else { return self.leaf(ty) };
                let element = self.local_ty();
                let list = Box::new(self.expr(&element.clone().list()));
                Node::Map {
                    list,
                    func: self.lambda(element, of),
                    items: ItemShape::default(),
                }
            }
            Move::Filter => {
                let Ty::List(of) = ty else { return self.leaf(ty) };
                Node::Filter {
                    list: Box::new(self.expr(ty)),
                    func: self.lambda((**of).clone(), &Ty::Boolean),
                }
            }
            Move::Sort => {
                let Ty::List(of) = ty else { return self.leaf(ty) };
                Node::Sort {
                    list: Box::new(self.expr(ty)),
                    key: self.lambda((**of).clone(), &Ty::Number),
                    descending: self.coin(),
                }
            }
            Move::Record => {
                let Ty::Record(r) = ty else { return self.leaf(ty) };
                let fields = self.records[*r].clone();
                let fields: BTreeMap<String, Node> = fields
                    .iter()
                    .map(|(name, field)| (name.clone(), self.expr(field)))
                    .collect();
                Node::Record {
                    ty: format!("R{r}"),
                    fields,
                }
            }
            Move::Find => {
                let Ty::Optional(of) = ty else { return self.leaf(ty) };
                // Over a `List<T?>` too: `find` of one is `T?`, not `T??`.
                let element = if self.coin() { ty.clone() } else { (**of).clone() };
                Node::Find {
                    list: Box::new(self.expr(&element.clone().list())),
                    func: self.lambda(element, &Ty::Boolean),
                }
            }
            Move::OrNothing => {
                let Ty::Optional(of) = ty else { return self.leaf(ty) };
                let cond = Box::new(self.expr(&Ty::Boolean));
                let present = Box::new(self.expr(of));
                let nothing = Box::new(Self::nothing());
                if self.coin() {
                    Node::Condition {
                        cond,
                        then: present,
                        otherwise: nothing,
                    }
                } else {
                    Node::Condition {
                        cond,
                        then: nothing,
                        otherwise: present,
                    }
                }
            }
            Move::Deep => {
                let mut node = self.leaf(&Ty::Number);
                for _ in 0..=self.pick(48) {
                    node = Node::UnaryOp {
                        op: UnaryOperator::Neg,
                        arg: Box::new(node),
                    };
                }
                node
            }
        }
    }

    /// A list of some type and a `Boolean` function of its items, for `all` and `any`.
    fn predicate(&mut self) -> (Box<Node>, Lambda) {
        let element = self.local_ty();
        let list = Box::new(self.expr(&element.clone().list()));
        (list, self.lambda(element, &Ty::Boolean))
    }

    /// A type `is_empty` takes: an optional, a list or text.
    fn emptiable(&mut self) -> Ty {
        match self.pick(3) {
            0 => Ty::Text,
            1 => self.local_ty().list(),
            _ => self.local_ty().optional(),
        }
    }

    /// Arguments for the value built-in `name`, in the types of one of its signatures (`language/14`).
    fn args(&mut self, name: &str) -> Vec<Node> {
        let numbers = Ty::Number.list();
        match name {
            "length" => {
                let arg = if self.coin() { Ty::Text } else { self.local_ty().list() };
                vec![self.expr(&arg)]
            }
            "is_empty" => {
                let arg = self.emptiable();
                vec![self.expr(&arg)]
            }
            "sum" | "maximum" | "minimum" => vec![self.expr(&numbers)],
            "contains" => {
                let element = self.local_ty();
                vec![self.expr(&element.clone().list()), self.expr(&element)]
            }
            "concat" => (0..2).map(|_| self.expr(&Ty::Text)).collect(),
            // Mostly bounds in order and a whole seed and index, which have an answer; sometimes any (R-BLT-02).
            "clamp" if !self.rarely(4) => {
                let low = self.small(-10, 10);
                let high = low + self.small(0, 10);
                vec![self.expr(&Ty::Number), Self::whole(low), Self::whole(high)]
            }
            "random" if !self.rarely(4) => vec![Self::whole(self.small(-50, 50)), Self::whole(self.small(0, 50))],
            "clamp" => (0..3).map(|_| self.expr(&Ty::Number)).collect(),
            "random" => (0..2).map(|_| self.expr(&Ty::Number)).collect(),
            // Mostly a small count, sometimes any, or one at or past the most a list may hold (R-TYP-24).
            "range" if self.rarely(12) => {
                let count = RANGES[self.pick(RANGES.len())];
                let value: Json = serde_json::from_str(count).expect("each of RANGES is JSON");
                vec![Node::Literal {
                    ty: Type::Number {},
                    value: LiteralValue::from(value),
                }]
            }
            "range" if !self.rarely(4) => vec![Self::whole(self.small(0, 8))],
            // `abs`, `floor`, `ceil`, `round`, `to_text` and a small `range`.
            _ => vec![self.expr(&Ty::Number)],
        }
    }

    /// The program: its record types and the goal's signature.
    fn source(&self, output: &Ty) -> String {
        let mut source = "language: velme/0.1\n\n".to_owned();
        for (r, fields) in self.records.iter().enumerate() {
            source.push_str(&format!("type R{r}:\n"));
            for (name, ty) in fields {
                source.push_str(&format!("    {name}: {}\n", ty.spelled()));
            }
            source.push('\n');
        }
        let params: Vec<String> = self
            .inputs
            .iter()
            .map(|(n, ty)| format!("{n}: {}", ty.spelled()))
            .collect();
        source.push_str(&format!(
            "goal {GOAL}({}) -> {}:\n    plan: \"Generated.\"\n",
            params.join(", "),
            output.spelled()
        ));
        source
    }

    fn document(&self, output: &Ty, body: Node) -> String {
        let types = self.usable.iter().map(|r| {
            let fields = self.records[*r].iter().map(|(name, ty)| (name.clone(), ty.ir()));
            (
                format!("R{r}"),
                RecordType {
                    fields: fields.collect(),
                },
            )
        });
        let goal = Goal {
            ir_version: IR_VERSION.to_owned(),
            builtins_version: BUILTINS_VERSION.to_owned(),
            goal: GOAL.to_owned(),
            types: types.collect(),
            inputs: self.inputs.iter().map(|(n, ty)| (n.clone(), ty.ir())).collect(),
            output: output.ir(),
            calls: Vec::new(),
            body,
        };
        serde_json::to_string(&goal).expect("IR serializes")
    }
}
