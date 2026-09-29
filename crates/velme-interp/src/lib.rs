//! The Velme reference interpreter: what IR means (`runtime/30` §3, P-4). Pure: it reaches no I/O, clock, randomness
//! or global state (INV-4, R-RUN-05), and meters every step in Velme fuel and every value in bytes (R-RUN-04, §7.1).
#![forbid(unsafe_code)]

mod eval;

use std::sync::Arc;

use velme_builtins::Value;
use velme_builtins::limits::{FUEL_PER_MS, MAX_FUEL, MAX_MEMORY};
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::ValidIr;

pub use eval::{Evaluator, NoProbe, Probe};

/// How many fuel units an evaluation spends between two looks at its [`Interrupt`]: what the fixed conversion of
/// `runtime/30` R-RUN-16 counts as a millisecond. The looks are at fixed points of the fuel meter, so an evaluation
/// that isn't interrupted is the same wherever and however fast it runs.
pub const POLL_FUEL: u64 = FUEL_PER_MS;

/// What one goal invocation may spend (`runtime/30` §7, R-RUN-16): its `max_fuel` and `max_memory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Velme fuel (R-RUN-04).
    pub fuel: u64,
    /// Bytes allocated (§7.1).
    pub memory: u64,
}

impl Limits {
    /// The system caps.
    pub const SYSTEM: Limits = Limits {
        fuel: MAX_FUEL,
        memory: MAX_MEMORY,
    };
}

/// What an invocation has spent so far, against its [`Limits`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spent {
    /// Velme fuel.
    pub fuel: u64,
    /// Cumulative bytes allocated (D-53).
    pub memory: u64,
}

/// A question asked of the outside every [`POLL_FUEL`] of fuel: has the run been stopped? It is how the wall-clock
/// watchdog reaches an evaluation without the interpreter holding a clock (`runtime/30` R-RUN-05, D-10): a run that
/// is never stopped never notices it.
#[derive(Clone)]
pub struct Interrupt(Arc<dyn Fn() -> bool + Send + Sync>);

impl Interrupt {
    /// An interrupt that fires once `stopped` returns `true`.
    pub fn new(stopped: impl Fn() -> bool + Send + Sync + 'static) -> Interrupt {
        Interrupt(Arc::new(stopped))
    }

    fn stopped(&self) -> bool {
        (self.0)()
    }
}

impl std::fmt::Debug for Interrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interrupt")
    }
}

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
    let goal = ir.goal();
    if inputs.len() != goal.inputs.len() || bindings.len() != goal.calls.len() {
        return Err(Error::Builtin(velme_builtins::Error::Internal).into());
    }
    let mut evaluator = Evaluator::from_budget(budget);
    for ((name, _), value) in goal.inputs.iter().zip(inputs) {
        evaluator.bind_input(name, value);
    }
    for (velme_ir::CallNode::Call(call), value) in goal.calls.iter().zip(bindings) {
        evaluator.bind_local(&call.binding, value);
    }
    let value = evaluator.eval(ir.body())?;
    let spent = evaluator.spent();
    Ok(Output {
        value,
        fuel: spent.fuel,
        memory: spent.memory,
    })
}

/// The arguments of the `index`th call of `ir`, evaluated on `inputs` and on the `bindings` known so far: one slot per
/// call in `calls` order, `None` for a call that has not run. An argument names only inputs and earlier bindings
/// (R-IR-09), so the slots it reads are filled when the scheduler asks (`runtime/30` R-RUN-06). The arguments are
/// expressions of the goal's body, so they spend from the invocation's limits after what it has already spent
/// (R-RUN-17): the arguments and what has been spent in all.
pub fn call_args(
    ir: &ValidIr,
    index: usize,
    inputs: &[Value],
    bindings: &[Option<Value>],
    budget: Budget,
) -> Result<(Vec<Value>, Spent), Failure> {
    let goal = ir.goal();
    if inputs.len() != goal.inputs.len() || bindings.len() != goal.calls.len() || index >= goal.calls.len() {
        return Err(Error::Builtin(velme_builtins::Error::Internal).into());
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
        .collect::<Result<Vec<_>, _>>()?;
    Ok((args, evaluator.spent()))
}

/// Why evaluation stopped without a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A built-in or operator had no answer: `VL0602`, `VL0606`, or `VL0607` for IR the validator should have rejected.
    Builtin(velme_builtins::Error),
    /// `VL0601`: the invocation needed more than its `max_fuel` (R-RUN-16).
    OutOfFuel {
        /// The limit it hit.
        max_fuel: u64,
    },
    /// `VL0604`: the invocation allocated more than its `max_memory` (R-RUN-16, §7.1).
    OutOfMemory {
        /// The limit it hit.
        max_memory: u64,
    },
    /// `VL0603`: the run was stopped from outside, by the wall-clock watchdog (D-10).
    Interrupted,
}

/// A failed evaluation: the error, and where in the lists being visited it happened (`language/14` R-BLT-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What went wrong.
    pub error: Error,
    /// The index of the element each enclosing collection node was visiting, innermost first.
    pub elements: Vec<usize>,
}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        Failure {
            error,
            elements: Vec::new(),
        }
    }
}

impl From<velme_builtins::Error> for Failure {
    fn from(error: velme_builtins::Error) -> Failure {
        Error::Builtin(error).into()
    }
}

impl Failure {
    /// The diagnostic code.
    pub fn code(&self) -> Code {
        match &self.error {
            Error::Builtin(error) => error.code(),
            Error::OutOfFuel { .. } => Code::BudgetExceeded,
            Error::OutOfMemory { .. } => Code::MemoryLimitExceeded,
            Error::Interrupted => Code::Timeout,
        }
    }

    /// The failure of the goal `goal` declared at `span`, worded as in `reference/90`.
    pub fn diagnostic(&self, goal: &str, span: Span) -> Diagnostic {
        let diag = match &self.error {
            Error::Builtin(velme_builtins::Error::Arithmetic { op }) => Diagnostic::new(
                Code::ArithmeticError,
                span,
                format!("`{goal}` tried to {op}, which has no answer."),
            ),
            Error::Builtin(velme_builtins::Error::ListTooLong { length }) => Diagnostic::new(
                Code::SizeLimitExceeded,
                span,
                format!("`{goal}` made a list or answer that's too big."),
            )
            .with_note(format!(
                "it asked for a list of {length} items; at most {} are allowed",
                velme_builtins::limits::MAX_LIST_SIZE
            )),
            // The evaluator reports a built-in that ran out of fuel as `OutOfFuel`, so it never arrives wrapped.
            Error::Builtin(
                velme_builtins::Error::Internal | velme_builtins::Error::OutOfFuel | velme_builtins::Error::OutOfMemory,
            ) => {
                return Diagnostic::internal_error();
            }
            Error::OutOfFuel { max_fuel } => Diagnostic::new(
                Code::BudgetExceeded,
                span,
                format!("`{goal}` took too many steps and was stopped."),
            )
            .with_note(format!("it may take {max_fuel} steps (fuel)")),
            Error::OutOfMemory { max_memory } => Diagnostic::new(
                Code::MemoryLimitExceeded,
                span,
                format!("`{goal}` needed more memory than it's allowed."),
            )
            .with_note(format!("it may allocate {max_memory} bytes")),
            Error::Interrupted => {
                Diagnostic::new(Code::Timeout, span, format!("`{goal}` ran too long and was stopped."))
            }
        };
        self.elements.iter().rev().fold(diag, |diag, i| {
            diag.with_note(format!("this happened at item {i} of a list (counting from 0)"))
        })
    }
}
