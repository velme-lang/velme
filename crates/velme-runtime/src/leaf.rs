//! Running a leaf goal from its locked artifact on the reference interpreter or, for `run`, `test` and `trace`, on the
//! backend of `--backend`, with its checks (`runtime/30` §4 steps
//! 6–7) and, for `velme test`, its examples (`language/12` R-GOAL-22). A goal with calls runs its body the same way once
//! the scheduler has run its calls ([`run_body`]).

use velme_builtins::Value;
use velme_builtins::limits::MAX_OUTPUT_BYTES;
use velme_check::{GoalChecks, Invocation};
use velme_diagnostics::{Code, Diagnostic};
use velme_interp::{Budget, Interrupt, Limits, Spent};
use velme_ir::{ValidIr, contract_key, encode_value};
use velme_sema::hir::{Goal, GoalId, GoalKind, Program};
use velme_synth::{ChildRunner, Verdict, verify};

use std::sync::Arc;

use crate::backend::{Backend, eval_leaf};
use crate::build::invoke_registry;
use crate::clock::Watchdog;
use crate::locked::LockedGoal;
use crate::registry::Registry;
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
    run_body(
        program,
        goal,
        source,
        locked,
        inputs,
        Vec::new(),
        Progress::default(),
        true,
    )
    .result
}

/// The limits of one invocation of `goal`: its effective `max_fuel` and `max_memory` (R-RUN-16).
pub(crate) fn limits(goal: &Goal) -> Limits {
    Limits {
        fuel: goal.budget.max_fuel,
        memory: goal.budget.max_memory,
    }
}

/// What an invocation brings to its body: what it already spent evaluating its calls' arguments, the run's watchdog,
/// if it has one, and the backend of a leaf's body.
#[derive(Debug, Clone, Default)]
pub(crate) struct Progress {
    pub(crate) spent: Spent,
    pub(crate) interrupt: Option<Interrupt>,
    pub(crate) backend: Backend,
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
/// Without `checked` the checks are left to the caller: verification judges an example before the checks
/// (`compiler/22` §6).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_body(
    program: &Program,
    goal: GoalId,
    source: &str,
    locked: &LockedGoal,
    inputs: Vec<Value>,
    bindings: Vec<Value>,
    progress: Progress,
    checked: bool,
) -> Body {
    if !checked {
        return match invoke(program, goal, locked, inputs, bindings, progress) {
            Ok(invocation) => Body {
                fuel: invocation.fuel,
                memory: invocation.memory,
                result: Ok(invocation.result),
                checks: Vec::new(),
            },
            Err(stopped) => Body::failed(stopped.0, stopped.1),
        };
    }
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

/// What `velme test` ran for a goal that passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tested {
    /// The `examples:` items run.
    pub examples: usize,
    /// The generated inputs run besides them (`compiler/22` §7).
    pub generated_inputs: usize,
}

/// Runs the examples of `goal`, leaf or composite, then its generated-input suite, through the verifier `velme build` uses
/// (R-GOAL-22, `compiler/22` §6, `tooling/40` R-CLI-04, D-107): every goal it calls runs from `registry` too. Every failing
/// example is reported, each with its inputs, expected and received values, in source order; the generated inputs run only
/// when the examples all pass, and stop at the first failure.
pub fn test_goal(
    program: &Program,
    goal: GoalId,
    source: &str,
    registry: &Registry,
    options: &Options,
) -> Result<Tested, Vec<Diagnostic>> {
    let internal = || vec![Diagnostic::internal_error()];
    let target = program.goals.get(goal.0).ok_or_else(internal)?;
    let locked = registry.get(goal).ok_or_else(internal)?;
    let runner = RegistryRunner {
        program,
        goal,
        source,
        registry,
        options,
    };
    let mut checks = GoalChecks::new(program, goal, source).map_err(|d| vec![d])?;
    let examples = checks.examples().to_vec();
    let mut failures = Vec::new();
    for example in &examples {
        // Each example is its own run, with its own wall-clock allowance (D-51).
        checks.watch(Some(Arc::new(Watchdog::start(options)).interrupt()));
        let invocation = match runner.run(&locked.ir, example.args.clone()) {
            Ok(invocation) => invocation,
            Err(found) => {
                failures.extend(found);
                continue;
            }
        };
        failures.extend(checks.judge_example(example, &invocation));
        match checks.run(&invocation) {
            Ok(checked) => failures.extend(checked.failures()),
            Err(diag) => failures.push(diag),
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    let contract = contract_key(program, goal).map_err(|_| internal())?;
    checks.watch(None);
    match verify(program, goal, source, contract, &locked.ir, &runner) {
        Verdict::Accepted(verified) => Ok(Tested {
            examples: usize::try_from(verified.examples).unwrap_or(usize::MAX),
            generated_inputs: usize::try_from(verified.generated_inputs).unwrap_or(usize::MAX),
        }),
        Verdict::Rejected(rejection) => {
            let d = if rejection.cause.code == Code::CheckFailed {
                Diagnostic::new(
                    Code::CheckFailed,
                    target.span,
                    format!("`{}` didn't pass its check: `{}`.", target.name, rejection.cause.name),
                )
            } else {
                // Not `VL0503`, which is a build's: a run that failed on an input is that failure, with its own code (D-107).
                Diagnostic::new(
                    rejection.cause.code,
                    target.span,
                    format!("`{}` failed on a generated input: {}.", target.name, rejection.line),
                )
            };
            Err(vec![d.with_note(format!("input: {}", rejection.input))])
        }
        Verdict::Watchdog(diagnostic) => Err(vec![diagnostic]),
        Verdict::Internal => Err(internal()),
    }
}

/// Runs a goal from the artifacts of a registry, as verification needs it (`compiler/22` R-SYNTH-14): the candidate it is
/// asked to run is the goal's own locked one.
struct RegistryRunner<'a> {
    program: &'a Program,
    goal: GoalId,
    source: &'a str,
    registry: &'a Registry,
    options: &'a Options,
}

impl ChildRunner for RegistryRunner<'_> {
    fn run(&self, _candidate: &ValidIr, inputs: Vec<Value>) -> Result<Invocation, Vec<Diagnostic>> {
        invoke_registry(
            self.program,
            self.goal,
            self.source,
            self.registry,
            self.options,
            inputs,
        )
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
    // A leaf's body is the backend's; a body with calls, after its arguments, is the interpreter's (R-SBX-17).
    let (value, spent) = if target.kind == GoalKind::Leaf && bindings.is_empty() && progress.spent == Spent::default() {
        let limits = limits(target);
        eval_leaf(
            &progress.backend,
            target,
            &locked.ir,
            inputs.clone(),
            limits,
            progress.interrupt,
        )
    } else {
        let budget = Budget::new(limits(target))
            .after(progress.spent)
            .watched(progress.interrupt);
        let (value, spent) = velme_interp::run_measured(&locked.ir, inputs.clone(), bindings.clone(), budget);
        (
            value.map_err(|failure| failure.diagnostic(&target.name, target.span)),
            spent,
        )
    };
    let value = value.map_err(|diagnostic| Box::new((diagnostic, spent)))?;
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
