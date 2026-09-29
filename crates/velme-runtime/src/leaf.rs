//! Running a leaf goal from its locked artifact on the reference interpreter, with its checks (`runtime/30` §4 steps
//! 6–7) and, for `velme test`, its examples (`language/12` R-GOAL-22). Goals with calls wait for the scheduler.

use velme_builtins::Value;
use velme_check::{GoalChecks, Invocation};
use velme_diagnostics::Diagnostic;
use velme_sema::hir::{GoalId, GoalKind, Program};

use crate::locked::LockedGoal;

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
    let checks = GoalChecks::new(program, goal, source).map_err(|d| vec![d])?;
    let invocation = invoke(program, goal, locked, inputs).map_err(|d| vec![d])?;
    let failures = checks.run(&invocation).map_err(|d| vec![d])?.failures();
    if failures.is_empty() {
        Ok(invocation.result)
    } else {
        Err(failures)
    }
}

/// Runs every example of the leaf goal `goal` with its locked IR, and its checks on each example's invocation
/// (R-GOAL-22). How many examples ran if all passed; otherwise every failure, example by example in source order.
pub fn test_leaf(program: &Program, goal: GoalId, source: &str, locked: &LockedGoal) -> Result<usize, Vec<Diagnostic>> {
    let checks = GoalChecks::new(program, goal, source).map_err(|d| vec![d])?;
    let mut failures = Vec::new();
    for example in checks.examples() {
        let invocation = match invoke(program, goal, locked, example.args.clone()) {
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

/// The goal's body evaluated on `inputs`, within its fuel budget (R-RUN-16).
fn invoke(program: &Program, goal: GoalId, locked: &LockedGoal, inputs: Vec<Value>) -> Result<Invocation, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    if target.kind != GoalKind::Leaf {
        return Err(Diagnostic::internal_error());
    }
    let output = velme_interp::run(&locked.ir, inputs.clone(), Vec::new(), target.budget.max_fuel)
        .map_err(|failure| failure.diagnostic(&target.name, target.span))?;
    Ok(Invocation {
        inputs,
        bindings: Vec::new(),
        result: output.value,
        fuel: output.fuel,
    })
}
