//! The Velme reference interpreter: what IR means (`runtime/30` §3, P-4). Pure: it reaches no I/O, clock, randomness
//! or global state (INV-4, R-RUN-05), and meters every step in Velme fuel (R-RUN-04).
#![forbid(unsafe_code)]

mod eval;

use velme_builtins::Value;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::ValidIr;

pub use eval::{Evaluator, NoProbe, Probe};

/// A goal body's value and the fuel it took (`runtime/30` §4 step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The value.
    pub value: Value,
    /// The fuel spent.
    pub fuel: u64,
}

/// Evaluates the `body` of `ir` on `inputs`, one per declared input in order, and `bindings`, one per call in `calls`
/// order, with at most `max_fuel` fuel (`runtime/30` §4 step 6, R-RUN-16). The runtime has already decoded the inputs
/// against the signature and run the calls.
pub fn run(ir: &ValidIr, inputs: Vec<Value>, bindings: Vec<Value>, max_fuel: u64) -> Result<Output, Failure> {
    let goal = ir.goal();
    if inputs.len() != goal.inputs.len() || bindings.len() != goal.calls.len() {
        return Err(Error::Builtin(velme_builtins::Error::Internal).into());
    }
    let mut evaluator = Evaluator::new(max_fuel);
    for ((name, _), value) in goal.inputs.iter().zip(inputs) {
        evaluator.bind_input(name, value);
    }
    for (velme_ir::CallNode::Call(call), value) in goal.calls.iter().zip(bindings) {
        evaluator.bind_local(&call.binding, value);
    }
    let value = evaluator.eval(ir.body())?;
    Ok(Output {
        value,
        fuel: evaluator.fuel(),
    })
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
            Error::Builtin(velme_builtins::Error::Internal | velme_builtins::Error::OutOfFuel) => {
                return Diagnostic::internal_error();
            }
            Error::OutOfFuel { max_fuel } => Diagnostic::new(
                Code::BudgetExceeded,
                span,
                format!("`{goal}` took too many steps and was stopped."),
            )
            .with_note(format!("it may take {max_fuel} steps (fuel)")),
        };
        self.elements.iter().rev().fold(diag, |diag, i| {
            diag.with_note(format!("this happened at item {i} of a list (counting from 0)"))
        })
    }
}
