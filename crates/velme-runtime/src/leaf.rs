//! Running a leaf goal from its locked artifact on the reference interpreter, with its checks (`runtime/30` §4 steps
//! 6–7) and, for `velme test`, its examples (`language/12` R-GOAL-22). A goal with calls runs its body the same way once
//! the scheduler has run its calls ([`run_body`]).

use velme_builtins::Value;
use velme_builtins::limits::MAX_OUTPUT_BYTES;
use velme_check::{GoalChecks, Invocation};
use velme_diagnostics::{Code, Diagnostic};
use velme_interp::{Budget, Interrupt, Limits, Spent};
use velme_ir::encode_value;
use velme_sema::hir::{GoalId, GoalKind, Program};

use std::sync::Arc;

use crate::clock::Watchdog;
use crate::locked::LockedGoal;
use crate::sched::Options;

/// Runs the leaf goal `goal` of `program`, whose source is `source`, on `inputs` with its locked IR, then evaluates its
/// checks on the invocation. Its value if every check holds; otherwise the failure, or every failed check in source
/// order (R-CHK-09).
pub fn run_leaf(
    program: &Program,
    goal: GoalId,
    source: &str,
    locked: &LockedGoal,
    inputs: Vec<Value>,
) -> Result<Value, Vec<Diagnostic>> {
    let target = program
        .goals
        .get(goal.0)
        .ok_or_else(|| vec![Diagnostic::internal_error()])?;
    if target.kind != GoalKind::Leaf {
        return Err(vec![Diagnostic::internal_error()]);
    }
    run_body(program, goal, source, locked, inputs, Vec::new(), Progress::default())
}

/// What an invocation brings to its body: what it already spent evaluating its calls' arguments, and the run's
/// watchdog, if it has one.
#[derive(Debug, Clone, Default)]
pub(crate) struct Progress {
    pub(crate) spent: Spent,
    pub(crate) interrupt: Option<Interrupt>,
}

/// Runs the body of `goal` on `inputs` and, for a goal with calls, on `bindings`, one value per call in block order,
/// then its checks on the invocation (`runtime/30` §4 steps 6–7), `progress` being how far the invocation already is.
/// Its value if every check holds; otherwise the
/// failure, or every failed check in source order (R-CHK-09).
pub(crate) fn run_body(
    program: &Program,
    goal: GoalId,
    source: &str,
    locked: &LockedGoal,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    progress: Progress,
) -> Result<Value, Vec<Diagnostic>> {
    let checks = GoalChecks::new(program, goal, source)
        .map_err(|d| vec![d])?
        .watched(progress.interrupt.clone());
    let invocation = invoke(program, goal, locked, inputs, bindings, progress).map_err(|d| vec![d])?;
    let failures = checks.run(&invocation).map_err(|d| vec![d])?.failures();
    if failures.is_empty() {
        Ok(invocation.result)
    } else {
        Err(failures)
    }
}

/// Runs every example of the leaf goal `goal` with its locked IR, and its checks on each example's invocation
/// (R-GOAL-22). How many examples ran if all passed; otherwise every failure, example by example in source order.
pub fn test_leaf(
    program: &Program,
    goal: GoalId,
    source: &str,
    locked: &LockedGoal,
    options: &Options,
) -> Result<usize, Vec<Diagnostic>> {
    let target = program
        .goals
        .get(goal.0)
        .ok_or_else(|| vec![Diagnostic::internal_error()])?;
    if target.kind != GoalKind::Leaf {
        return Err(vec![Diagnostic::internal_error()]);
    }
    let watchdog = Arc::new(Watchdog::start(options));
    let interrupt = Some(watchdog.interrupt());
    let checks = GoalChecks::new(program, goal, source)
        .map_err(|d| vec![d])?
        .watched(interrupt.clone());
    let mut failures = Vec::new();
    for example in checks.examples() {
        let invocation = match invoke(
            program,
            goal,
            locked,
            example.args.clone(),
            Vec::new(),
            Progress {
                spent: Spent::default(),
                interrupt: interrupt.clone(),
            },
        ) {
            Ok(invocation) => invocation,
            Err(diag) => {
                failures.push(diag);
                continue;
            }
        };
        failures.extend(checks.judge_example(example, &invocation));
        match checks.run(&invocation) {
            Ok(checked) => failures.extend(checked.failures()),
            Err(diag) => failures.push(diag),
        }
    }
    if failures.is_empty() {
        Ok(checks.examples().len())
    } else {
        Err(failures)
    }
}

/// The goal's body evaluated on `inputs`, within its fuel and memory budget (R-RUN-16) and with an answer of at most
/// `max_output_bytes` of JSON (`runtime/30` §7).
fn invoke(
    program: &Program,
    goal: GoalId,
    locked: &LockedGoal,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    progress: Progress,
) -> Result<Invocation, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    let limits = Limits {
        fuel: target.budget.max_fuel,
        memory: target.budget.max_memory,
    };
    let budget = Budget::new(limits).after(progress.spent).watched(progress.interrupt);
    let output = velme_interp::run_after(&locked.ir, inputs.clone(), bindings.clone(), budget)
        .map_err(|failure| failure.diagnostic(&target.name, target.span))?;
    if encode_value(&output.value).is_err() {
        return Err(Diagnostic::new(
            Code::SizeLimitExceeded,
            target.span,
            format!("`{}` made a list or answer that's too big.", target.name),
        )
        .with_note(format!("its answer is more than {MAX_OUTPUT_BYTES} bytes as JSON")));
    }
    Ok(Invocation {
        inputs,
        bindings,
        result: output.value,
        fuel: output.fuel,
        memory: output.memory,
    })
}
