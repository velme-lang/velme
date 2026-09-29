//! Expression evaluation (`runtime/30` R-RUN-01..05): strict, left to right, every step charged in Velme fuel.

use std::collections::BTreeMap;

use velme_builtins::{Builtin, Function, Value, equals, sort_by_fuel, sort_order};
use velme_ir::{BinaryOperator, Lambda, Node, RecordType, ReduceLambda, Trusted, UnaryOperator};

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

/// The record types of an evaluator that has evaluated nothing yet.
static NO_TYPES: BTreeMap<String, RecordType> = BTreeMap::new();

/// Evaluates IR expressions over named inputs and locals, charging one fuel budget across every evaluation it runs
/// (`runtime/30` R-RUN-04, R-RUN-17). It evaluates only [`Trusted`] IR: a validated goal's body, or a lowered check or
/// example that passed the validator (INV-1).
#[derive(Debug)]
pub struct Evaluator<'ir> {
    /// The record types of the expression being evaluated, by name (`compiler/21` R-IR-01).
    types: &'ir BTreeMap<String, RecordType>,
    inputs: Vec<(&'ir str, Value)>,
    /// Call bindings, then `let` names and lambda parameters as they come into scope; names never repeat (R-IR-12).
    locals: Vec<(&'ir str, Value)>,
    fuel: u64,
    max_fuel: u64,
}

type Eval = Result<Value, Failure>;

impl<'ir> Evaluator<'ir> {
    /// An evaluator with no names in scope that may spend `max_fuel`.
    pub fn new(max_fuel: u64) -> Self {
        Evaluator {
            types: &NO_TYPES,
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

    /// The value of `expr`.
    pub fn eval(&mut self, expr: Trusted<'ir>) -> Eval {
        self.eval_probed(expr, &mut NoProbe)
    }

    /// The value of `expr`, showing `probe` every value computed on the way and every element visited.
    pub fn eval_probed<P: Probe>(&mut self, expr: Trusted<'ir>, probe: &mut P) -> Eval {
        self.types = expr.types();
        self.probed(expr.node(), probe)
    }

    /// The value of `node`, a part of the expression being evaluated.
    fn probed<P: Probe>(&mut self, node: &'ir Node, probe: &mut P) -> Eval {
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

    /// The fuel left to spend.
    fn left(&self) -> u64 {
        self.max_fuel.saturating_sub(self.fuel)
    }

    /// Charges what a metered call cost beyond the `charged` fuel already charged for it; a call that stopped at the
    /// budget it was given is out of fuel.
    fn settle(&mut self, output: Result<velme_builtins::Output, velme_builtins::Error>, charged: u64) -> Eval {
        match output {
            Ok(output) => {
                self.charge(output.fuel.saturating_sub(charged))?;
                Ok(output.value)
            }
            Err(velme_builtins::Error::OutOfFuel) => {
                self.fuel = self.max_fuel.saturating_add(1);
                Err(Error::OutOfFuel {
                    max_fuel: self.max_fuel,
                }
                .into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Each node costs one fuel unit before anything else (R-RUN-04).
    fn node<P: Probe>(&mut self, node: &'ir Node, probe: &mut P) -> Eval {
        self.charge(1)?;
        match node {
            // Decoded once, by the validator (D-83); evaluating it allocates nothing (§7.1).
            Node::Literal { value, .. } => value.decoded().cloned().ok_or_else(internal),
            Node::Input { name } => lookup(&self.inputs, name),
            Node::Local { name } => lookup(&self.locals, name),
            Node::Record { ty, fields } => self.record(ty, fields, probe),
            Node::List { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.probed(item, probe))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::list(items))
            }
            Node::FieldGet { of, field } => match self.probed(of, probe)? {
                Value::Record(record) => record.get(field).cloned().ok_or_else(internal),
                _ => Err(internal()),
            },
            Node::BinaryOp { op, left, right } => self.binary(*op, left, right, probe),
            Node::UnaryOp { op, arg } => {
                let arg = self.probed(arg, probe)?;
                match (op, arg) {
                    (UnaryOperator::Neg, Value::Number(x)) => Ok(Value::Number(-x)),
                    (UnaryOperator::Not, Value::Boolean(b)) => Ok(Value::Boolean(!b)),
                    // The built-in's own unit is this node's (D-60).
                    (UnaryOperator::IsEmpty, arg) => {
                        let output = Function::IsEmpty.call(&[arg], self.left().saturating_add(1));
                        self.settle(output, 1)
                    }
                    _ => Err(internal()),
                }
            }
            Node::Let { bind, body } => {
                for (name, value) in bind {
                    let value = self.probed(value, probe)?;
                    self.locals.push((name, value));
                }
                self.probed(body, probe)
            }
            Node::Condition { cond, then, otherwise } => {
                if truth(self.probed(cond, probe)?)? {
                    self.probed(then, probe)
                } else {
                    self.probed(otherwise, probe)
                }
            }
            // `default` only when it is needed, so a narrowed path never evaluates it (R-IR-05).
            Node::Narrow { of, default } => match self.probed(of, probe)? {
                Value::Nothing => self.probed(default, probe),
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
                    .map(|arg| self.probed(arg, probe))
                    .collect::<Result<Vec<_>, _>>()?;
                // The catalog cost already counts this node's unit (D-52).
                let output = function.call(&args, self.left().saturating_add(1));
                self.settle(output, 1)
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
            values.push((name.clone(), self.probed(field, probe)?));
        }
        Ok(Value::record(ty, values))
    }

    fn binary<P: Probe>(&mut self, op: BinaryOperator, left: &'ir Node, right: &'ir Node, probe: &mut P) -> Eval {
        let left = self.probed(left, probe)?;
        if let BinaryOperator::And | BinaryOperator::Or = op {
            let left = truth(left)?;
            // `and` stops at `false`, `or` at `true`.
            if left == (op == BinaryOperator::Or) {
                return Ok(Value::Boolean(left));
            }
            return Ok(Value::Boolean(truth(self.probed(right, probe)?)?));
        }
        let right = self.probed(right, probe)?;
        if let BinaryOperator::Eq | BinaryOperator::Ne = op {
            // Metered as it compares, so it stops where the fuel runs out (D-83).
            let output = equals(&left, &right, self.left());
            let equal = truth(self.settle(output, 0)?)?;
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
        match self.probed(node, probe)? {
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
        let value = self.probed(&func.body, probe);
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
        // The list first, then `init` (R-RUN-02).
        let list = self.list(list, probe)?;
        let mut acc = self.probed(init, probe)?;
        for (i, element) in list.iter().enumerate() {
            self.charge(1)?;
            probe.element(node, i, element);
            let scope = self.locals.len();
            self.locals.push((&func.acc, acc));
            self.locals.push((&func.param, element.clone()));
            let next = self.probed(&func.body, probe);
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

#[cfg(test)]
mod tests {
    use velme_builtins::limits::MAX_FUEL;
    use velme_builtins::{BUILTINS_VERSION, Value};
    use velme_ir::IR_VERSION;
    use velme_test_support::{program, valid_ir};

    use super::Evaluator;

    /// Whatever a failed evaluation brought into scope leaves with it, and what was bound before stays: the `let`
    /// fails with `a` bound, the `map` inside its lambda with `x` bound.
    #[test]
    fn an_evaluator_is_reusable_after_a_failure() {
        let program =
            program("language: velme/0.1\n\ngoal Compute(n: Number) -> Number:\n    plan: \"Something of n.\"\n");
        let number = |n: &str| format!(r#"{{"kind": "literal", "type": {{"t": "Number"}}, "value": {n}}}"#);
        let div = |left: &str| {
            format!(
                r#"{{"kind": "binary", "op": "div", "left": {left}, "right": {}}}"#,
                number("0")
            )
        };
        let failing_let = format!(
            r#"{{"kind": "let", "bind": [["a", {}], ["b", {}]], "body": {{"kind": "local", "name": "a"}}}}"#,
            number("1"),
            div(&number("1"))
        );
        let failing_map = format!(
            r#"{{"kind": "builtin", "name": "sum", "args": [{{"kind": "map",
                "list": {{"kind": "builtin", "name": "range", "args": [{}]}},
                "fn": {{"param": "x", "body": {}}}}}]}}"#,
            number("2"),
            div(r#"{"kind": "local", "name": "x"}"#)
        );
        let irs: Vec<_> = [failing_let, failing_map]
            .iter()
            .map(|body| {
                let doc = format!(
                    r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Compute",
                        "types": {{}}, "inputs": [["n", {{"t": "Number"}}]], "output": {{"t": "Number"}}, "body": {body}}}"#
                );
                valid_ir(&program, &doc)
            })
            .collect();
        let mut evaluator = Evaluator::new(MAX_FUEL);
        evaluator.bind_local("before", Value::Boolean(true));
        for ir in &irs {
            assert!(evaluator.eval(ir.body()).is_err());
            assert_eq!(evaluator.locals, [("before", Value::Boolean(true))]);
        }
    }
}
