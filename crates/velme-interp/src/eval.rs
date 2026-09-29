//! Expression evaluation (`runtime/30` R-RUN-01..05): strict, left to right, every step charged in Velme fuel.

use std::collections::BTreeMap;

use velme_builtins::{Builtin, Function, Value, sort_by_fuel, sort_order};
use velme_ir::{BinaryOperator, Lambda, Node, RecordType, ReduceLambda, UnaryOperator, decode_literal};

use crate::{Error, Failure};

/// Sees what an evaluation computes, for reports that show the values behind a result (`language/13` R-CHK-10).
pub trait Probe {
    /// `node` evaluated to `value`.
    fn value(&mut self, node: &Node, value: &Value);

    /// The collection node `node` is about to visit `element`, at `index` of its list.
    fn element(&mut self, node: &Node, index: usize, element: &Value);

    /// `node` failed: first the node where the failure arose, then each enclosing node.
    fn failed(&mut self, node: &Node, failure: &Failure);
}

/// A [`Probe`] that looks at nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProbe;

impl Probe for NoProbe {
    fn value(&mut self, _: &Node, _: &Value) {}

    fn element(&mut self, _: &Node, _: usize, _: &Value) {}

    fn failed(&mut self, _: &Node, _: &Failure) {}
}

/// Evaluates IR expressions over named inputs and locals, charging one fuel budget across every evaluation it runs
/// (`runtime/30` R-RUN-04, R-RUN-17).
#[derive(Debug)]
pub struct Evaluator<'ir> {
    /// The record types in scope, by name (`compiler/21` R-IR-01).
    types: &'ir BTreeMap<String, RecordType>,
    inputs: Vec<(&'ir str, Value)>,
    /// Call bindings, then `let` names and lambda parameters as they come into scope; names never repeat (R-IR-12).
    locals: Vec<(&'ir str, Value)>,
    fuel: u64,
    max_fuel: u64,
}

type Eval = Result<Value, Failure>;

impl<'ir> Evaluator<'ir> {
    /// An evaluator with no names in scope, over the record types `types`, that may spend `max_fuel`.
    pub fn new(types: &'ir BTreeMap<String, RecordType>, max_fuel: u64) -> Self {
        Evaluator {
            types,
            inputs: Vec::new(),
            locals: Vec::new(),
            fuel: 0,
            max_fuel,
        }
    }

    /// The same evaluator having already spent `fuel` of its `max_fuel`: the rest of an invocation's budget, for its
    /// checks (`language/13` R-CHK-08).
    pub fn with_fuel_spent(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    /// Binds the input `name`.
    pub fn bind_input(&mut self, name: &'ir str, value: Value) {
        self.inputs.push((name, value));
    }

    /// Binds the local `name`: a call binding, or `result` (`language/13` R-CHK-02).
    pub fn bind_local(&mut self, name: &'ir str, value: Value) {
        self.locals.push((name, value));
    }

    /// The fuel spent so far.
    pub fn fuel(&self) -> u64 {
        self.fuel
    }

    /// The value of `node`.
    pub fn eval(&mut self, node: &'ir Node) -> Eval {
        self.eval_probed(node, &mut NoProbe)
    }

    /// The value of `node`, showing `probe` every value computed on the way and every element visited.
    pub fn eval_probed<P: Probe>(&mut self, node: &'ir Node, probe: &mut P) -> Eval {
        // Whatever a node brings into scope leaves with it, on failure too, so the evaluator stays usable.
        let scope = self.locals.len();
        let value = self.node(node, probe);
        self.locals.truncate(scope);
        match value {
            Ok(value) => {
                probe.value(node, &value);
                Ok(value)
            }
            Err(failure) => {
                probe.failed(node, &failure);
                Err(failure)
            }
        }
    }

    fn charge(&mut self, fuel: u64) -> Result<(), Failure> {
        self.fuel = self.fuel.saturating_add(fuel);
        if self.fuel > self.max_fuel {
            return Err(Error::OutOfFuel {
                max_fuel: self.max_fuel,
            }
            .into());
        }
        Ok(())
    }

    /// Each node costs one fuel unit before anything else (R-RUN-04).
    fn node<P: Probe>(&mut self, node: &'ir Node, probe: &mut P) -> Eval {
        self.charge(1)?;
        match node {
            Node::Literal { ty, value } => decode_literal(value, ty, self.types).ok_or_else(internal),
            Node::Input { name } => lookup(&self.inputs, name),
            Node::Local { name } => lookup(&self.locals, name),
            Node::Record { ty, fields } => self.record(ty, fields, probe),
            Node::List { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.eval_probed(item, probe))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::list(items))
            }
            Node::FieldGet { of, field } => match self.eval_probed(of, probe)? {
                Value::Record(record) => record.get(field).cloned().ok_or_else(internal),
                _ => Err(internal()),
            },
            Node::BinaryOp { op, left, right } => self.binary(*op, left, right, probe),
            Node::UnaryOp { op, arg } => {
                let arg = self.eval_probed(arg, probe)?;
                match (op, arg) {
                    (UnaryOperator::Neg, Value::Number(x)) => Ok(Value::Number(-x)),
                    (UnaryOperator::Not, Value::Boolean(b)) => Ok(Value::Boolean(!b)),
                    // The built-in's own unit is this node's (D-60).
                    (UnaryOperator::IsEmpty, arg) => Ok(Function::IsEmpty.call(&[arg])?.value),
                    _ => Err(internal()),
                }
            }
            Node::Let { bind, body } => {
                for (name, value) in bind {
                    let value = self.eval_probed(value, probe)?;
                    self.locals.push((name, value));
                }
                self.eval_probed(body, probe)
            }
            Node::Condition { cond, then, otherwise } => {
                if truth(self.eval_probed(cond, probe)?)? {
                    self.eval_probed(then, probe)
                } else {
                    self.eval_probed(otherwise, probe)
                }
            }
            // `default` only when it is needed, so a narrowed path never evaluates it (R-IR-05).
            Node::Narrow { of, default } => match self.eval_probed(of, probe)? {
                Value::Nothing => self.eval_probed(default, probe),
                present => Ok(present),
            },
            Node::Map { list, func } => {
                let list = self.list(list, probe)?;
                let mut out = Vec::with_capacity(list.len());
                for (i, element) in list.iter().enumerate() {
                    out.push(self.visit(node, func, i, element, probe)?);
                }
                Ok(Value::list(out))
            }
            Node::Filter { list, func } => {
                let list = self.list(list, probe)?;
                let mut out = Vec::new();
                for (i, element) in list.iter().enumerate() {
                    if truth(self.visit(node, func, i, element, probe)?)? {
                        out.push(element.clone());
                    }
                }
                Ok(Value::list(out))
            }
            Node::Find { list, func } => {
                let list = self.list(list, probe)?;
                for (i, element) in list.iter().enumerate() {
                    if truth(self.visit(node, func, i, element, probe)?)? {
                        return Ok(element.clone());
                    }
                }
                Ok(Value::Nothing)
            }
            // Short-circuit at the first element that decides the answer (D-58).
            Node::All { list, func } | Node::Any { list, func } => {
                let decides = matches!(node, Node::Any { .. });
                let list = self.list(list, probe)?;
                for (i, element) in list.iter().enumerate() {
                    if truth(self.visit(node, func, i, element, probe)?)? == decides {
                        return Ok(Value::Boolean(decides));
                    }
                }
                Ok(Value::Boolean(!decides))
            }
            Node::Reduce { list, init, func } => self.reduce(node, list, init, func, probe),
            Node::Sort { list, key, descending } => self.sort(node, list, key, *descending, probe),
            Node::Builtin { name, args } => {
                let function = Builtin::find(name).and_then(|b| b.function).ok_or_else(internal)?;
                let args = args
                    .iter()
                    .map(|arg| self.eval_probed(arg, probe))
                    .collect::<Result<Vec<_>, _>>()?;
                let output = function.call(&args)?;
                // The catalog cost already counts this node's unit (D-52).
                self.charge(output.fuel.saturating_sub(1))?;
                Ok(output.value)
            }
            Node::Call(_) => Err(internal()),
        }
    }

    /// Fields in declaration order, not the order the node lists them in (R-RUN-02).
    fn record<P: Probe>(&mut self, ty: &str, fields: &'ir BTreeMap<String, Node>, probe: &mut P) -> Eval {
        let types = self.types;
        let declared = types.get(ty).ok_or_else(internal)?;
        let mut values = Vec::with_capacity(declared.fields.len());
        for (name, _) in &declared.fields {
            let field = fields.get(name).ok_or_else(internal)?;
            values.push((name.clone(), self.eval_probed(field, probe)?));
        }
        Ok(Value::record(ty, values))
    }

    fn binary<P: Probe>(&mut self, op: BinaryOperator, left: &'ir Node, right: &'ir Node, probe: &mut P) -> Eval {
        let left = self.eval_probed(left, probe)?;
        if let BinaryOperator::And | BinaryOperator::Or = op {
            let left = truth(left)?;
            // `and` stops at `false`, `or` at `true`.
            if left == (op == BinaryOperator::Or) {
                return Ok(Value::Boolean(left));
            }
            return Ok(Value::Boolean(truth(self.eval_probed(right, probe)?)?));
        }
        let right = self.eval_probed(right, probe)?;
        if let BinaryOperator::Eq | BinaryOperator::Ne = op {
            let mut leaves = 0;
            let equal = compare(&left, &right, &mut leaves);
            if composite(&left) || composite(&right) {
                self.charge(leaves)?;
            }
            return Ok(Value::Boolean(equal == (op == BinaryOperator::Eq)));
        }
        let (Value::Number(a), Value::Number(b)) = (left, right) else {
            return Err(internal());
        };
        Ok(match op {
            BinaryOperator::Add => Value::Number(a.checked_add(b)?),
            BinaryOperator::Sub => Value::Number(a.checked_sub(b)?),
            BinaryOperator::Mul => Value::Number(a.checked_mul(b)?),
            BinaryOperator::Div => Value::Number(a.checked_div(b)?),
            BinaryOperator::Lt => Value::Boolean(a < b),
            BinaryOperator::Le => Value::Boolean(a <= b),
            BinaryOperator::Gt => Value::Boolean(a > b),
            BinaryOperator::Ge => Value::Boolean(a >= b),
            BinaryOperator::Eq | BinaryOperator::Ne | BinaryOperator::And | BinaryOperator::Or => {
                return Err(internal());
            }
        })
    }

    /// The items of the list `node` evaluates to.
    fn list<P: Probe>(&mut self, node: &'ir Node, probe: &mut P) -> Result<std::sync::Arc<[Value]>, Failure> {
        match self.eval_probed(node, probe)? {
            Value::List(items) => Ok(items),
            _ => Err(internal()),
        }
    }

    /// `func` applied to the element at `index` of the list of `node`: one more fuel unit per element visited
    /// (R-RUN-04), and a failure names the element (R-BLT-07).
    fn visit<P: Probe>(
        &mut self,
        node: &Node,
        func: &'ir Lambda,
        index: usize,
        element: &Value,
        probe: &mut P,
    ) -> Eval {
        self.charge(1)?;
        probe.element(node, index, element);
        let scope = self.locals.len();
        self.locals.push((&func.param, element.clone()));
        let value = self.eval_probed(&func.body, probe);
        self.locals.truncate(scope);
        value.map_err(|mut failure| {
            failure.elements.push(index);
            failure
        })
    }

    fn reduce<P: Probe>(
        &mut self,
        node: &Node,
        list: &'ir Node,
        init: &'ir Node,
        func: &'ir ReduceLambda,
        probe: &mut P,
    ) -> Eval {
        let list = self.list(list, probe)?;
        let mut acc = self.eval_probed(init, probe)?;
        for (i, element) in list.iter().enumerate() {
            self.charge(1)?;
            probe.element(node, i, element);
            let scope = self.locals.len();
            self.locals.push((&func.acc, acc));
            self.locals.push((&func.param, element.clone()));
            let next = self.eval_probed(&func.body, probe);
            self.locals.truncate(scope);
            acc = next.map_err(|mut failure| {
                failure.elements.push(i);
                failure
            })?;
        }
        Ok(acc)
    }

    /// Every key first, in list order, then a stable sort (R-BLT-11).
    fn sort<P: Probe>(
        &mut self,
        node: &Node,
        list: &'ir Node,
        key: &'ir Lambda,
        descending: bool,
        probe: &mut P,
    ) -> Eval {
        let list = self.list(list, probe)?;
        let mut keys = Vec::with_capacity(list.len());
        for (i, element) in list.iter().enumerate() {
            // `visit` charges the key evaluation's unit.
            match self.visit(node, key, i, element, probe)? {
                Value::Number(k) => keys.push(k),
                _ => return Err(internal()),
            }
        }
        let n = u64::try_from(list.len()).unwrap_or(u64::MAX);
        // The formula's own unit and one per key are already charged (D-52).
        self.charge(sort_by_fuel(n).saturating_sub(1 + n))?;
        let sorted = sort_order(&keys, descending)
            .into_iter()
            .map(|i| list.get(i).cloned().ok_or_else(internal))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Value::list(sorted))
    }
}

/// The `VL0607` failure of IR the validator should have rejected.
fn internal() -> Failure {
    velme_builtins::Error::Internal.into()
}

fn lookup(scope: &[(&str, Value)], name: &str) -> Eval {
    scope
        .iter()
        .rev()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.clone())
        .ok_or_else(internal)
}

fn truth(value: Value) -> Result<bool, Failure> {
    match value {
        Value::Boolean(b) => Ok(b),
        _ => Err(internal()),
    }
}

fn composite(value: &Value) -> bool {
    matches!(value, Value::List(_) | Value::Record(_))
}

/// Structural equality (R-TYP-20, D-61), counting the scalar leaves compared up to the first difference (R-RUN-04).
/// Lists of different lengths differ before any leaf is compared.
fn compare(a: &Value, b: &Value, leaves: &mut u64) -> bool {
    match (a, b) {
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| compare(a, b, leaves))
        }
        (Value::Record(a), Value::Record(b)) => {
            a.name == b.name
                && a.fields.len() == b.fields.len()
                && a.fields
                    .iter()
                    .zip(&b.fields)
                    .all(|((_, a), (_, b))| compare(a, b, leaves))
        }
        _ => {
            *leaves += 1;
            a == b
        }
    }
}
