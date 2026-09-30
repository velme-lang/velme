//! The Velme reference interpreter: what IR means (`runtime/30` §3, P-4). Pure: it reaches no I/O, clock, randomness
//! or global state (INV-4, R-RUN-05), and meters every step in Velme fuel and every value in bytes (R-RUN-04, §7.1).
#![forbid(unsafe_code)]

mod eval;

use velme_builtins::Value;
use velme_builtins::limits::FUEL_PER_MS;
use velme_ir::ValidIr;

pub use eval::{Evaluator, NoProbe, Probe};
// Shared with the WASM backend, so both report the same failures in the same words (D-114).
pub use velme_builtins::execution::{Error, Failure, Interrupt, Limits, Spent};

/// How many fuel units an evaluation spends between two looks at its [`Interrupt`]: what the fixed conversion of
/// `runtime/30` R-RUN-16 counts as a millisecond. The looks are at fixed points of the fuel meter, so an evaluation
/// that isn't interrupted is the same wherever and however fast it runs.
pub const POLL_FUEL: u64 = FUEL_PER_MS;

/// What an evaluation may spend, what it has already spent, and whether anything can stop it from outside.
#[derive(Debug, Clone)]
pub struct Budget {
    /// The invocation's limits.
    pub limits: Limits,
    /// What it has spent before this evaluation: on the arguments of its calls, or on its body.
    pub spent: Spent,
    /// The watchdog, if the run has one.
    pub interrupt: Option<Interrupt>,
}

impl Budget {
    /// A fresh budget within `limits`, with no watchdog.
    pub fn new(limits: Limits) -> Budget {
        Budget {
            limits,
            spent: Spent::default(),
            interrupt: None,
        }
    }

    /// The same budget having already spent `spent` (`runtime/30` R-RUN-17).
    pub fn after(mut self, spent: Spent) -> Budget {
        self.spent = spent;
        self
    }

    /// The same budget stopped from outside by `interrupt`.
    pub fn watched(mut self, interrupt: Option<Interrupt>) -> Budget {
        self.interrupt = interrupt;
        self
    }
}

/// A goal body's value and what it took (`runtime/30` §4 step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The value.
    pub value: Value,
    /// The fuel spent.
    pub fuel: u64,
    /// The bytes allocated.
    pub memory: u64,
}

/// Evaluates the `body` of `ir` on `inputs`, one per declared input in order, and `bindings`, one per call in `calls`
/// order, within `limits` (`runtime/30` §4 step 6, R-RUN-16). The runtime has already decoded the inputs against the
/// signature and run the calls.
pub fn run(ir: &ValidIr, inputs: Vec<Value>, bindings: Vec<Value>, limits: Limits) -> Result<Output, Failure> {
    run_after(ir, inputs, bindings, Budget::new(limits))
}

/// [`run`] for an invocation that has already spent `budget.spent` of its limits, evaluating the arguments of its
/// calls (`runtime/30` R-RUN-17): the body spends from what is left, and [`Output`] counts all of it.
pub fn run_after(ir: &ValidIr, inputs: Vec<Value>, bindings: Vec<Value>, budget: Budget) -> Result<Output, Failure> {
    let (value, spent) = run_measured(ir, inputs, bindings, budget);
    Ok(Output {
        value: value?,
        fuel: spent.fuel,
        memory: spent.memory,
    })
}

/// [`run_after`], returning what the invocation had spent in all whether the body finished or failed, so a trace can
/// show the work of a failed goal (`runtime/30` §8).
pub fn run_measured(
    ir: &ValidIr,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    budget: Budget,
) -> (Result<Value, Failure>, Spent) {
    let spent = budget.spent;
    let goal = ir.goal();
    if inputs.len() != goal.inputs.len() || bindings.len() != goal.calls.len() {
        return (Err(Error::Builtin(velme_builtins::Error::Internal).into()), spent);
    }
    let mut evaluator = Evaluator::from_budget(budget);
    for ((name, _), value) in goal.inputs.iter().zip(inputs) {
        evaluator.bind_input(name, value);
    }
    for (velme_ir::CallNode::Call(call), value) in goal.calls.iter().zip(bindings) {
        evaluator.bind_local(&call.binding, value);
    }
    let value = evaluator.eval(ir.body());
    (value, evaluator.spent())
}

/// The arguments of the `index`th call of `ir`, evaluated on `inputs` and on the `bindings` known so far: one slot per
/// call in `calls` order, `None` for a call that has not run. An argument names only inputs and earlier bindings
/// (R-IR-09), so the slots it reads are filled when the scheduler asks (`runtime/30` R-RUN-06). The arguments are
/// expressions of the goal's body, so they spend from the invocation's limits after what it has already spent
/// (R-RUN-17): the arguments and what has been spent in all. What was spent is returned whether the arguments
/// evaluated or not, like [`run_measured`].
pub fn call_args(
    ir: &ValidIr,
    index: usize,
    inputs: &[Value],
    bindings: &[Option<Value>],
    budget: Budget,
) -> (Result<Vec<Value>, Failure>, Spent) {
    let goal = ir.goal();
    if inputs.len() != goal.inputs.len() || bindings.len() != goal.calls.len() || index >= goal.calls.len() {
        return (
            Err(Error::Builtin(velme_builtins::Error::Internal).into()),
            budget.spent,
        );
    }
    let mut evaluator = Evaluator::from_budget(budget);
    for ((name, _), value) in goal.inputs.iter().zip(inputs) {
        evaluator.bind_input(name, value.clone());
    }
    for (velme_ir::CallNode::Call(call), value) in goal.calls.iter().zip(bindings) {
        if let Some(value) = value {
            evaluator.bind_local(&call.binding, value.clone());
        }
    }
    let args = ir
        .call_args(index)
        .into_iter()
        .map(|arg| evaluator.eval(arg))
        .collect::<Result<Vec<_>, _>>();
    (args, evaluator.spent())
}
