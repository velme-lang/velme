//! The execution types both backends share (D-114): what an invocation may spend and has spent, how it is stopped
//! from outside, and how it fails, with the wording of each failure (`reference/90`), so the interpreter and the
//! WASM backend report the same failure in the same words (INV-3).

use std::sync::Arc;

use velme_diagnostics::{Code, Diagnostic, Span};

use crate::limits::{MAX_FUEL, MAX_LIST_SIZE, MAX_MEMORY};

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

/// A question asked of the outside at fixed points of the fuel meter: has the run been stopped? It is how the
/// wall-clock watchdog reaches an evaluation without the interpreter holding a clock (`runtime/30` R-RUN-05, D-10): a
/// run that is never stopped never notices it.
#[derive(Clone)]
pub struct Interrupt(Arc<dyn Fn() -> bool + Send + Sync>);

impl Interrupt {
    /// An interrupt that fires once `stopped` returns `true`.
    pub fn new(stopped: impl Fn() -> bool + Send + Sync + 'static) -> Interrupt {
        Interrupt(Arc::new(stopped))
    }

    /// Whether the run has been stopped.
    pub fn stopped(&self) -> bool {
        (self.0)()
    }
}

impl std::fmt::Debug for Interrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interrupt")
    }
}

/// Why evaluation stopped without a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A built-in or operator had no answer: `VL0602`, `VL0606`, or `VL0607` for IR the validator should have rejected.
    Builtin(crate::Error),
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

impl From<crate::Error> for Failure {
    fn from(error: crate::Error) -> Failure {
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
            Error::Builtin(crate::Error::Arithmetic { op }) => Diagnostic::new(
                Code::ArithmeticError,
                span,
                format!("`{goal}` tried to {op}, which has no answer."),
            ),
            Error::Builtin(crate::Error::ListTooLong { length }) => Diagnostic::new(
                Code::SizeLimitExceeded,
                span,
                format!("`{goal}` made a list or answer that's too big."),
            )
            .with_note(format!(
                "it asked for a list of {length} items; at most {} are allowed",
                MAX_LIST_SIZE
            )),
            // The evaluator reports a built-in that ran out of fuel as `OutOfFuel`, so it never arrives wrapped.
            Error::Builtin(crate::Error::Internal | crate::Error::OutOfFuel | crate::Error::OutOfMemory) => {
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
