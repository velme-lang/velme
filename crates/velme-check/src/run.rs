//! The CheckRunner (`runtime/30` §2, D-80): check items and examples evaluated on the reference interpreter, whichever
//! executor ran the goal, and the `VL0501`/`VL0502` reports of `language/13` §5.

use std::collections::HashMap;

use velme_builtins::Value;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_interp::{Error, Evaluator, Failure, Probe};
use velme_ir::{CheckScope, Node, TrustedExpr, display_value};
use velme_sema::hir::{BinaryOp, Expr, ExprKind, Goal, GoalId, Program, Quantifier};

use crate::lower::{Lowered, lower_check, lower_example, result_local};

/// One goal invocation, as its checks observe it (R-CHK-02, R-CHK-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// One value per parameter.
    pub inputs: Vec<Value>,
    /// One value per `call` binding, in block order; a wired goal's last one is its `result`.
    pub bindings: Vec<Value>,
    /// The goal's output.
    pub result: Value,
    /// The fuel the invocation has spent; its checks spend from what is left of its budget (R-CHK-08).
    pub fuel: u64,
}

/// What evaluating a goal's checks on one invocation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// One report per check item, in source order (R-CHK-05).
    pub items: Vec<ItemReport>,
    /// The fuel the checks spent.
    pub fuel: u64,
}

impl Checked {
    /// The `VL0501` of each failed item, in source order; the goal's failure cites the first (R-CHK-09).
    pub fn failures(&self) -> Vec<Diagnostic> {
        self.items.iter().filter_map(|item| item.failure.clone()).collect()
    }
}

/// How one check item went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemReport {
    /// The item, as written.
    pub span: Span,
    /// For a failed item, its paths and helper calls with their values, and the operands it never evaluated
    /// (R-CHK-10); empty for an item that holds.
    pub parts: Vec<Part>,
    /// Its `VL0501` if it failed.
    pub failure: Option<Diagnostic>,
}

/// A piece of a check item and what it evaluated to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Where it is.
    pub span: Span,
    /// Its source text.
    pub text: String,
    /// Its whole value, or `None` if it was not evaluated (R-CHK-06).
    pub value: Option<Value>,
}

/// An example of a goal (`language/12` R-GOAL-21), ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleCase {
    /// The inputs, one per parameter.
    pub args: Vec<Value>,
    /// The output it expects.
    pub expected: Value,
    /// The example, as written.
    pub span: Span,
    /// The call as written: `Double(2)`.
    pub given: String,
}

/// A goal's checks and examples, lowered once (R-CHK-11), to evaluate on any number of its invocations.
#[derive(Debug)]
pub struct GoalChecks<'p> {
    source: &'p str,
    goal: &'p Goal,
    checks: Vec<Lowered>,
    examples: Vec<ExampleCase>,
}

impl<'p> GoalChecks<'p> {
    /// The checks and examples of `goal`, from the checked `program` whose source is `source`.
    pub fn new(program: &'p Program, goal: GoalId, source: &'p str) -> Result<Self, Diagnostic> {
        let goal = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
        let scope = CheckScope::new(program, goal)?;
        let checks = goal
            .checks
            .iter()
            .map(|check| lower_check(program, goal, &scope, check))
            .collect::<Result<Vec<_>, _>>()?;
        let mut examples = Vec::with_capacity(goal.examples.len());
        for example in &goal.examples {
            let (args, expected) = lower_example(program, goal, &scope, example)?;
            let text = text(source, example.span);
            // The call is what comes before the `==`.
            let given = text
                .get(..example.expected.span.start.saturating_sub(example.span.start))
                .unwrap_or(text)
                .trim_end()
                .trim_end_matches("==")
                .trim_end();
            examples.push(ExampleCase {
                args: args.iter().map(|arg| literal(&arg.node)).collect::<Result<_, _>>()?,
                expected: literal(&expected.node)?,
                span: example.span,
                given: given.to_owned(),
            });
        }
        Ok(GoalChecks {
            source,
            goal,
            checks,
            examples,
        })
    }

    /// The goal's examples, in source order; they run before generated inputs (`language/12` R-GOAL-22).
    pub fn examples(&self) -> &[ExampleCase] {
        &self.examples
    }

    /// `VL0502` if `invocation`, the goal run on `example`'s inputs, gave something other than it expects. Its checks
    /// are still to be run on the invocation (R-GOAL-22).
    pub fn judge_example(&self, example: &ExampleCase, invocation: &Invocation) -> Option<Diagnostic> {
        (invocation.result != example.expected).then(|| {
            Diagnostic::new(
                Code::ExampleFailed,
                example.span,
                format!(
                    "For {}, `{}` gave {} but the example expects {}.",
                    example.given,
                    self.goal.name,
                    display_value(&invocation.result),
                    display_value(&example.expected)
                ),
            )
        })
    }

    /// Evaluates every check item on `invocation`, in source order (R-CHK-05). An item that is false, or whose
    /// evaluation fails with `VL0602`, fails with `VL0501` (R-CHK-07). A budget failure — running out of fuel, or
    /// `VL0606` for a list past its limit — stops the checks with that failure instead (R-CHK-08); `VL0607` reports a
    /// bug.
    pub fn run(&self, invocation: &Invocation) -> Result<Checked, Diagnostic> {
        let mut evaluator = self.scope(invocation)?;
        let mut items = Vec::with_capacity(self.checks.len());
        for (check, lowered) in self.goal.checks.iter().zip(&self.checks) {
            let start = evaluator.fuel();
            let cause = match evaluator.eval(lowered.node.trusted()) {
                Ok(Value::Boolean(true)) => {
                    items.push(ItemReport {
                        span: check.span,
                        parts: Vec::new(),
                        failure: None,
                    });
                    continue;
                }
                Ok(Value::Boolean(false)) => None,
                Err(failure) => match failure.error {
                    Error::Builtin(velme_builtins::Error::Arithmetic { .. }) => Some(failure),
                    Error::OutOfFuel { .. } | Error::Builtin(velme_builtins::Error::ListTooLong { .. }) => {
                        return Err(failure.diagnostic(&self.goal.name, check.span));
                    }
                    Error::Builtin(velme_builtins::Error::Internal | velme_builtins::Error::OutOfFuel) => {
                        return Err(Diagnostic::internal_error());
                    }
                },
                Ok(_) => return Err(Diagnostic::internal_error()),
            };
            // Only a failed item is probed: again, from the same fuel, so it goes exactly as before.
            let report = self.probe(lowered, invocation, start)?;
            items.push(ItemReport {
                span: check.span,
                parts: report.parts(check),
                failure: Some(report.failure(self.goal, check, cause.as_ref(), invocation)),
            });
        }
        Ok(Checked {
            items,
            fuel: evaluator.fuel().saturating_sub(invocation.fuel),
        })
    }

    /// The parts of check item `index` evaluated on `invocation` (R-CHK-10), whether it holds or not: which values it
    /// saw, and which operands it never evaluated (R-CHK-06).
    pub fn explain(&self, index: usize, invocation: &Invocation) -> Result<Vec<Part>, Diagnostic> {
        let (check, lowered) = self
            .goal
            .checks
            .get(index)
            .zip(self.checks.get(index))
            .ok_or_else(Diagnostic::internal_error)?;
        Ok(self.probe(lowered, invocation, invocation.fuel)?.parts(check))
    }

    /// `lowered` evaluated again with a [`Recorder`], starting from `fuel` spent.
    fn probe(&self, lowered: &Lowered, invocation: &Invocation, fuel: u64) -> Result<Report<'p>, Diagnostic> {
        let mut recorder = Recorder::default();
        let mut evaluator = self.scope(invocation)?.with_fuel_spent(fuel);
        // The outcome is already known; the recorder holds what the report needs.
        let _ = evaluator.eval_probed(lowered.node.trusted(), &mut recorder);
        Ok(Report::new(self.source, lowered, recorder))
    }

    /// An evaluator with the check scope bound — inputs, bindings and `result` (R-CHK-02) — that has spent what the
    /// invocation spent of the goal's budget (R-CHK-08).
    fn scope(&self, invocation: &Invocation) -> Result<Evaluator<'_>, Diagnostic> {
        let goal = self.goal;
        if invocation.inputs.len() != goal.params.len() || invocation.bindings.len() != goal.bindings.len() {
            return Err(Diagnostic::internal_error());
        }
        let mut evaluator = Evaluator::new(goal.budget.max_fuel).with_fuel_spent(invocation.fuel);
        for (param, value) in goal.params.iter().zip(&invocation.inputs) {
            evaluator.bind_input(&param.name, value.clone());
        }
        // A wired goal's `result` binding is the output itself.
        for (binding, value) in goal.bindings.iter().zip(&invocation.bindings) {
            if binding.name != result_local() {
                evaluator.bind_local(&binding.name, value.clone());
            }
        }
        evaluator.bind_local(result_local(), invocation.result.clone());
        Ok(evaluator)
    }
}

/// The value of an example's literal (R-GOAL-21), which costs next to no fuel.
fn literal(expr: &TrustedExpr) -> Result<Value, Diagnostic> {
    Evaluator::new(velme_builtins::limits::MAX_FUEL)
        .eval(expr.trusted())
        .map_err(|_| Diagnostic::internal_error())
}

/// What one item's evaluation computed, by node, as of the element each enclosing collection node visited last: each
/// node's value, each collection node's element, and the nodes that failed, innermost first.
#[derive(Default)]
struct Recorder {
    values: HashMap<*const Node, Value>,
    elements: HashMap<*const Node, (usize, Value)>,
    failed: Vec<*const Node>,
}

impl Probe for Recorder {
    fn value(&mut self, node: &Node, value: &Value) {
        self.values.insert(node, value.clone());
    }

    fn element(&mut self, node: &Node, index: usize, element: &Value) {
        // What an earlier element left under the lambda would pass for this element's.
        if let Some(body) = node.lambda_body() {
            for inner in body.preorder() {
                let inner = std::ptr::from_ref(inner);
                self.values.remove(&inner);
                self.elements.remove(&inner);
                self.failed.retain(|n| *n != inner);
            }
        }
        self.elements.insert(node, (index, element.clone()));
    }

    fn failed(&mut self, node: &Node, _: &Failure) {
        self.failed.push(node);
    }
}

/// What happened to one source expression.
enum Seen<'r> {
    Value(&'r Value),
    Failed,
    NotEvaluated,
}

/// The report of one probed check item.
struct Report<'r> {
    source: &'r str,
    recorder: Recorder,
    /// The node each span lowered to: the first in pre-order, which is the one a source expression lowers to when the
    /// lowering added nodes around it.
    nodes: HashMap<Span, *const Node>,
    /// The span of each node.
    spans: HashMap<*const Node, Span>,
}

impl<'r> Report<'r> {
    fn new(source: &'r str, lowered: &Lowered, recorder: Recorder) -> Self {
        let mut nodes = HashMap::new();
        let mut spans = HashMap::new();
        for (node, span) in lowered.node.node().preorder().into_iter().zip(&lowered.spans) {
            let node = std::ptr::from_ref(node);
            nodes.entry(*span).or_insert(node);
            spans.insert(node, *span);
        }
        Report {
            source,
            recorder,
            nodes,
            spans,
        }
    }

    fn node(&self, e: &Expr) -> Option<*const Node> {
        self.nodes.get(&e.span).copied()
    }

    fn seen(&self, e: &Expr) -> Seen<'_> {
        let Some(node) = self.node(e) else {
            return Seen::NotEvaluated;
        };
        if let Some(value) = self.recorder.values.get(&node) {
            Seen::Value(value)
        } else if self.recorder.failed.contains(&node) {
            Seen::Failed
        } else {
            Seen::NotEvaluated
        }
    }

    /// The paths and helper calls of `check` with their values, and the operands never evaluated (R-CHK-10), each
    /// once.
    fn parts(&self, check: &Expr) -> Vec<Part> {
        let mut parts = Vec::new();
        self.collect(check, &mut parts);
        let mut unique: Vec<Part> = Vec::with_capacity(parts.len());
        for part in parts {
            if !unique.iter().any(|p| p.text == part.text && p.value == part.value) {
                unique.push(part);
            }
        }
        unique
    }

    fn collect(&self, e: &Expr, parts: &mut Vec<Part>) {
        let part = |value: Option<Value>| Part {
            span: e.span,
            text: text(self.source, e.span).to_owned(),
            value,
        };
        let value = match self.seen(e) {
            Seen::Value(value) => value,
            Seen::Failed => {
                self.children(e, parts);
                return;
            }
            // A literal holds no surprise, evaluated or not.
            Seen::NotEvaluated if is_literal(e) => return,
            Seen::NotEvaluated => {
                parts.push(part(None));
                return;
            }
        };
        match &e.kind {
            ExprKind::Input { .. }
            | ExprKind::Binding { .. }
            | ExprKind::Result
            | ExprKind::Var { .. }
            | ExprKind::Field { .. }
            | ExprKind::Project { .. } => parts.push(part(Some(value.clone()))),
            ExprKind::Builtin { args, .. } => {
                parts.push(part(Some(value.clone())));
                for arg in args {
                    self.collect(arg, parts);
                }
            }
            // A body's values are those of the element that decided the answer; without one they'd be arbitrary.
            ExprKind::Quantified { collection, body, .. } => {
                self.collect(collection, parts);
                if self.decider(e).is_some() {
                    self.collect(body, parts);
                }
            }
            _ => self.children(e, parts),
        }
    }

    fn children(&self, e: &Expr, parts: &mut Vec<Part>) {
        for child in children(e) {
            self.collect(child, parts);
        }
    }

    /// The element that decided the quantifier `e`: the first counterexample of a false `every`, the first witness of a
    /// true `some` (R-CHK-06).
    fn decider(&self, e: &Expr) -> Option<&(usize, Value)> {
        let ExprKind::Quantified { quantifier, .. } = &e.kind else {
            return None;
        };
        let node = self.node(e)?;
        let decides = match quantifier {
            Quantifier::Every => Value::Boolean(false),
            Quantifier::Some => Value::Boolean(true),
        };
        (self.recorder.values.get(&node) == Some(&decides))
            .then(|| self.recorder.elements.get(&node))
            .flatten()
    }

    /// The quantifiers of `e` that decided their answer, outermost first; one inside a quantifier's body only if that
    /// one decided too, since otherwise its element is not the one that mattered.
    fn deciders<'e>(&self, e: &'e Expr, out: &mut Vec<&'e Expr>) {
        if let ExprKind::Quantified { collection, body, .. } = &e.kind {
            self.deciders(collection, out);
            if self.decider(e).is_some() {
                out.push(e);
                self.deciders(body, out);
            }
            return;
        }
        for child in children(e) {
            self.deciders(child, out);
        }
    }

    /// The quantifiers `e`'s failure happened inside, outermost first, each with the element it was visiting
    /// (R-BLT-07).
    fn failed_in<'s, 'e>(&'s self, e: &'e Expr, out: &mut Vec<(&'e Expr, &'s (usize, Value))>) {
        if !matches!(self.seen(e), Seen::Failed) {
            return;
        }
        if let ExprKind::Quantified { .. } = &e.kind
            && let Some(element) = self.node(e).and_then(|n| self.recorder.elements.get(&n))
        {
            out.push((e, element));
        }
        for child in children(e) {
            self.failed_in(child, out);
        }
    }

    /// The `VL0501` of `check`, which was false or failed with `cause`.
    fn failure(&self, goal: &Goal, check: &Expr, cause: Option<&Failure>, invocation: &Invocation) -> Diagnostic {
        let mut diag = Diagnostic::new(
            Code::CheckFailed,
            check.span,
            format!(
                "`{}` didn't pass its check: `{}`.",
                goal.name,
                text(self.source, check.span)
            ),
        );
        if let Some((op, received, expected)) = self.comparison(self.deciding(check)) {
            diag = diag
                .with_note(format!("Expected: {op}{expected}"))
                .with_note(format!("Received: {received}"));
        }
        let mut deciders = Vec::new();
        self.deciders(check, &mut deciders);
        for q in deciders {
            let (
                Some((index, element)),
                ExprKind::Quantified {
                    quantifier,
                    collection,
                    body,
                    ..
                },
            ) = (self.decider(q), &q.kind)
            else {
                continue;
            };
            let verdict = match quantifier {
                Quantifier::Every => "fails",
                Quantifier::Some => "satisfies",
            };
            diag = diag.with_note(format!(
                "`{}[{index}]` = {} {verdict} `{}`",
                text(self.source, collection.span),
                display_value(element),
                text(self.source, body.span)
            ));
        }
        for part in self.parts(check) {
            diag = diag.with_note(match part.value {
                Some(value) => format!("`{}` = {}", part.text, display_value(&value)),
                None => format!("`{}` was not evaluated", part.text),
            });
        }
        if let Some(cause) = cause {
            let at = self
                .recorder
                .failed
                .first()
                .and_then(|node| self.spans.get(node))
                .copied()
                .unwrap_or(check.span);
            diag = diag.with_note(format!(
                "could not evaluate `{}`: {} [{}]",
                text(self.source, at),
                cause_text(cause),
                cause.code().as_str()
            ));
            let mut within = Vec::new();
            self.failed_in(check, &mut within);
            for (q, (index, element)) in within {
                if let ExprKind::Quantified { collection, .. } = &q.kind {
                    diag = diag.with_note(format!(
                        "this happened at `{}[{index}]` = {}",
                        text(self.source, collection.span),
                        display_value(element)
                    ));
                }
            }
        }
        for (param, value) in goal.params.iter().zip(&invocation.inputs) {
            diag = diag.with_note(format!("input `{}` = {}", param.name, display_value(value)));
        }
        for (binding, value) in goal.bindings.iter().zip(&invocation.bindings) {
            diag = diag.with_note(format!("call `{}` = {}", binding.name, display_value(value)));
        }
        diag
    }

    /// The part of a false `check` that made it false: through `and`, the operand that was false.
    fn deciding<'e>(&self, check: &'e Expr) -> &'e Expr {
        if let ExprKind::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } = &check.kind
        {
            let side = match self.seen(lhs) {
                Seen::Value(Value::Boolean(false)) => lhs,
                _ => rhs,
            };
            return self.deciding(side);
        }
        check
    }

    /// For a comparison with both sides evaluated: how the expected side is written, the received (left) value and the
    /// expected (right) value.
    fn comparison(&self, e: &Expr) -> Option<(&'static str, String, String)> {
        let ExprKind::Binary { op, lhs, rhs } = &e.kind else {
            return None;
        };
        let op = match op {
            BinaryOp::Eq => "",
            BinaryOp::NotEq => "not ",
            BinaryOp::Lt => "< ",
            BinaryOp::LtEq => "<= ",
            BinaryOp::Gt => "> ",
            BinaryOp::GtEq => ">= ",
            _ => return None,
        };
        let (Seen::Value(received), Seen::Value(expected)) = (self.seen(lhs), self.seen(rhs)) else {
            return None;
        };
        Some((op, display_value(received), display_value(expected)))
    }
}

/// The direct subexpressions of `e`, in source order.
fn children(e: &Expr) -> Vec<&Expr> {
    match &e.kind {
        ExprKind::Number { .. }
        | ExprKind::Text { .. }
        | ExprKind::Bool { .. }
        | ExprKind::Nothing
        | ExprKind::Input { .. }
        | ExprKind::Binding { .. }
        | ExprKind::Result
        | ExprKind::Var { .. } => Vec::new(),
        ExprKind::Builtin { args: items, .. } | ExprKind::Record { fields: items, .. } | ExprKind::List { items } => {
            items.iter().collect()
        }
        ExprKind::Field { base, .. } | ExprKind::Project { base, .. } => vec![base],
        ExprKind::Unary { operand, .. } | ExprKind::IsEmpty { operand, .. } => vec![operand],
        ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
        ExprKind::If { condition, then } => vec![condition, then],
        ExprKind::Quantified { collection, body, .. } => vec![collection, body],
    }
}

fn is_literal(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Number { .. } | ExprKind::Text { .. } | ExprKind::Bool { .. } | ExprKind::Nothing
    )
}

/// The source text at `span`.
fn text(source: &str, span: Span) -> &str {
    source.get(span.start..span.end).unwrap_or("?")
}

/// Why an item could not be evaluated, in `reference/90`'s words (only `VL0602` fails an item, R-CHK-07).
fn cause_text(cause: &Failure) -> String {
    match &cause.error {
        Error::Builtin(velme_builtins::Error::Arithmetic { op }) => format!("it tried to {op}, which has no answer"),
        Error::Builtin(_) | Error::OutOfFuel { .. } => "something went wrong inside Velme".to_owned(),
    }
}
