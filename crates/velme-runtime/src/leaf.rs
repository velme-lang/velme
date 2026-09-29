//! Running a leaf goal from its locked artifact on the reference interpreter, with its checks (`runtime/30` §4 steps
//! 6–7) and, for `velme test`, its examples (`language/12` R-GOAL-22). A goal with calls runs its body the same way once
//! the scheduler has run its calls ([`run_body`]).

use velme_builtins::Value;
use velme_builtins::limits::MAX_OUTPUT_BYTES;
use velme_check::{GoalChecks, Invocation};
use velme_diagnostics::{Code, Diagnostic};
use velme_interp::{Budget, Interrupt, Limits, Spent};
use velme_ir::encode_value;
use velme_sema::hir::{Goal, GoalId, GoalKind, Program};

use std::sync::Arc;

use crate::clock::Watchdog;
use crate::locked::LockedGoal;
use crate::sched::{CheckRun, Options};

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
    run_body(program, goal, source, locked, inputs, Vec::new(), Progress::default()).result
}

/// The limits of one invocation of `goal`: its effective `max_fuel` and `max_memory` (R-RUN-16).
pub(crate) fn limits(goal: &Goal) -> Limits {
    Limits {
        fuel: goal.budget.max_fuel,
        memory: goal.budget.max_memory,
    }
}

/// What an invocation brings to its body: what it already spent evaluating its calls' arguments, and the run's
/// watchdog, if it has one.
#[derive(Debug, Clone, Default)]
pub(crate) struct Progress {
    pub(crate) spent: Spent,
    pub(crate) interrupt: Option<Interrupt>,
}

/// What running a body left, for the goal's trace event (`runtime/30` §8 `goal`, `check`).
pub(crate) struct Body {
    /// Its value if every check holds; otherwise the failure, or every failed check in source order (R-CHK-09).
    pub(crate) result: Result<Value, Vec<Diagnostic>>,
    /// Fuel and memory the invocation used in all — its calls' arguments, body and checks, which share one budget
    /// (R-CHK-08) — up to where it stopped.
    pub(crate) fuel: u64,
    pub(crate) memory: u64,
    /// Every check item, in source order, if the checks ran.
    pub(crate) checks: Vec<CheckRun>,
}

impl Body {
    /// A body that could not run for a bug of ours.
    pub(crate) fn internal() -> Body {
        Body::failed(Diagnostic::internal_error(), Spent::default())
    }

    fn failed(diagnostic: Diagnostic, spent: Spent) -> Body {
        Body {
            result: Err(vec![diagnostic]),
            fuel: spent.fuel,
            memory: spent.memory,
            checks: Vec::new(),
        }
    }
}

/// Runs the body of `goal` on `inputs` and, for a goal with calls, on `bindings`, one value per call in block order,
/// then its checks on the invocation (`runtime/30` §4 steps 6–7), `progress` being how far the invocation already is.
pub(crate) fn run_body(
    program: &Program,
    goal: GoalId,
    source: &str,
    locked: &LockedGoal,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    progress: Progress,
) -> Body {
    let checks = match GoalChecks::new(program, goal, source) {
        Ok(checks) => checks.watched(progress.interrupt.clone()),
        Err(diagnostic) => return Body::failed(diagnostic, progress.spent),
    };
    let invocation = match invoke(program, goal, locked, inputs, bindings, progress) {
        Ok(invocation) => invocation,
        Err(stopped) => return Body::failed(stopped.0, stopped.1),
    };
    let (checked, spent) = checks.run_measured(&invocation);
    let (items, stopped) = match checked {
        Ok(checked) => (checked.items, None),
        Err(stopped) => (stopped.finished.clone(), Some(stopped)),
    };
    let failures = match &stopped {
        Some(stopped) => vec![stopped.diagnostic.clone()],
        None => items.iter().filter_map(|item| item.failure.clone()).collect(),
    };
    let ran = items
        .into_iter()
        .map(|item| CheckRun {
            text: source
                .get(item.span.start..item.span.end)
                .unwrap_or_default()
                .to_owned(),
            span: item.span,
            passed: item.failure.is_none(),
            values: item.parts,
        })
        .collect();
    Body {
        result: if failures.is_empty() {
            Ok(invocation.result)
        } else {
            Err(failures)
        },
        fuel: spent.fuel,
        memory: spent.memory,
        checks: ran,
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
    let mut checks = GoalChecks::new(program, goal, source).map_err(|d| vec![d])?;
    let examples = checks.examples().to_vec();
    let mut failures = Vec::new();
    for example in &examples {
        // Each example is its own run, with its own wall-clock allowance (D-51).
        let interrupt = Some(Arc::new(Watchdog::start(options)).interrupt());
        checks.watch(interrupt.clone());
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
            Err(stopped) => {
                failures.push(stopped.0);
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
        Ok(examples.len())
    } else {
        Err(failures)
    }
}

/// The goal's body evaluated on `inputs`, within its fuel and memory budget (R-RUN-16) and with an answer of at most
/// `max_output_bytes` of JSON (`runtime/30` §7). A failure comes with what the invocation had spent by then.
fn invoke(
    program: &Program,
    goal: GoalId,
    locked: &LockedGoal,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    progress: Progress,
) -> Result<Invocation, Box<(Diagnostic, Spent)>> {
    let target = program
        .goals
        .get(goal.0)
        .ok_or_else(|| Box::new((Diagnostic::internal_error(), progress.spent)))?;
    let budget = Budget::new(limits(target))
        .after(progress.spent)
        .watched(progress.interrupt);
    let (value, spent) = velme_interp::run_measured(&locked.ir, inputs.clone(), bindings.clone(), budget);
    let value = value.map_err(|failure| Box::new((failure.diagnostic(&target.name, target.span), spent)))?;
    if encode_value(&value).is_err() {
        let diagnostic = Diagnostic::new(
            Code::SizeLimitExceeded,
            target.span,
            format!("`{}` made a list or answer that's too big.", target.name),
        )
        .with_note(format!("its answer is more than {MAX_OUTPUT_BYTES} bytes as JSON"));
        return Err(Box::new((diagnostic, spent)));
    }
    Ok(Invocation {
        inputs,
        bindings,
        result: value,
        fuel: spent.fuel,
        memory: spent.memory,
    })
}
