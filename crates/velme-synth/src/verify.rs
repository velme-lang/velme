//! The verification pipeline (`compiler/22` §6): a validated candidate runs on every example and generated input through
//! the [`ChildRunner`], and its examples and checks are judged. Only a candidate that passes all of it is eligible for
//! the store (R-SYNTH-16).

use velme_builtins::Value;
use velme_check::{GoalChecks, Invocation};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Fingerprint, ValidIr, display_value};
use velme_sema::hir::{GoalId, Program};

use crate::attempt::{Cause, Rejection};
use crate::generate::{TestInput, test_inputs};
use crate::request::AttemptDiagnostic;

/// Runs a whole candidate goal on one input, its already-accepted children included, through the scheduler that runs
/// every goal (`compiler/22` R-SYNTH-14, D-98): `velme-runtime` implements it, so `velme-synth` never reaches the
/// artifact store. The invocation it returns has not had its checks run.
pub trait ChildRunner: Send + Sync {
    /// The invocation of `candidate` on `inputs`, or why it failed: a `VL06xx`, `VL0603` from the watchdog being one.
    fn run(&self, candidate: &ValidIr, inputs: Vec<Value>) -> Result<Invocation, Vec<Diagnostic>>;
}

/// What a candidate that passed verification was checked on (`runtime/32` §3 `verification`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// `examples:` run.
    pub examples: u64,
    /// Generated inputs the checks ran on.
    pub generated_inputs: u64,
    /// The fingerprint of the whole input set: the list of the inputs' canonical JSON.
    pub input_set: Fingerprint,
    /// The most fuel one run took, its checks included.
    pub max_fuel_observed: u64,
}

/// How verification ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every step passed.
    Accepted(Verified),
    /// A verdict on the candidate: it is not correct.
    Rejected(Rejection),
    /// The watchdog stopped a run (`VL0603`): never a verdict on the candidate (D-51, R-SYNTH-14).
    Watchdog(Diagnostic),
    /// A bug of ours (`VL0607`).
    Internal,
}

/// Verifies `candidate` for `goal` of `program`, whose source is `source` (`compiler/22` §6, §7).
pub fn verify(
    program: &Program,
    goal: GoalId,
    source: &str,
    contract_key: Fingerprint,
    candidate: &ValidIr,
    runner: &dyn ChildRunner,
) -> Verdict {
    let Ok(checks) = GoalChecks::new(program, goal, source) else {
        return Verdict::Internal;
    };
    let examples: Vec<Vec<Value>> = checks.examples().iter().map(|e| e.args.clone()).collect();
    let Ok(generated) = test_inputs(program, goal, contract_key, &examples) else {
        return Verdict::Internal;
    };
    let names: Vec<&str> = program
        .goals
        .get(goal.0)
        .map(|g| g.params.iter().map(|p| p.name.as_str()).collect())
        .unwrap_or_default();
    let mut max_fuel = 0;
    for (i, input) in generated.inputs.iter().enumerate() {
        let invocation = match runner.run(candidate, input.args.clone()) {
            Ok(invocation) => invocation,
            Err(found) => return stopped(found, input, &names),
        };
        if let Some(example) = checks.examples().get(i)
            && let Some(diagnostic) = checks.judge_example(example, &invocation)
        {
            return Verdict::Rejected(wrap(
                Code::ExampleFailed,
                example.given.clone(),
                &diagnostic,
                input,
                &names,
            ));
        }
        let (checked, spent) = checks.run_measured(&invocation);
        max_fuel = max_fuel.max(spent.fuel);
        match checked {
            Ok(checked) => {
                if let Some(failure) = checked.failures().into_iter().next() {
                    let name = text(source, &failure);
                    return Verdict::Rejected(wrap(Code::CheckFailed, name, &failure, input, &names));
                }
            }
            Err(stop) => return stopped(vec![stop.diagnostic], input, &names),
        }
    }
    let Ok(input_set) = Fingerprint::of(&generated.inputs.iter().map(|i| i.json.as_str()).collect::<Vec<_>>()) else {
        return Verdict::Internal;
    };
    Verdict::Accepted(Verified {
        examples: generated.examples as u64,
        generated_inputs: (generated.inputs.len() - generated.examples) as u64,
        input_set,
        max_fuel_observed: max_fuel,
    })
}

/// The source text a diagnostic points at.
fn text(source: &str, diagnostic: &Diagnostic) -> String {
    source
        .get(diagnostic.span.start..diagnostic.span.end)
        .unwrap_or_default()
        .to_owned()
}

/// A run that failed: the watchdog and bugs are not verdicts; anything else is `VL0503` with the failure as its cause.
fn stopped(found: Vec<Diagnostic>, input: &TestInput, names: &[&str]) -> Verdict {
    if let Some(d) = found.iter().find(|d| d.code == Code::Timeout) {
        return Verdict::Watchdog(d.clone());
    }
    let Some(first) = found.first() else {
        return Verdict::Internal;
    };
    if first.code == Code::InternalError {
        return Verdict::Internal;
    }
    Verdict::Rejected(wrap(first.code, String::new(), first, input, names))
}

/// `name is value, …` for `input`, with what Velme computed (R-SYNTH-19).
fn given(input: &TestInput, names: &[&str]) -> String {
    let parts: Vec<String> = names
        .iter()
        .zip(&input.args)
        .map(|(name, value)| format!("`{name}` is `{}`", display_value(value)))
        .collect();
    if parts.is_empty() {
        "no input".to_owned()
    } else {
        parts.join(" and ")
    }
}

/// A failed case as a `VL0503` cause (R-SYNTH-15): `inner` is why, `name` the check or example it names.
fn wrap(code: Code, name: String, inner: &Diagnostic, input: &TestInput, names: &[&str]) -> Rejection {
    let line = match code {
        Code::ExampleFailed => inner.message.clone(),
        Code::CheckFailed => format!("your check `{name}` fails when {}", given(input, names)),
        _ => format!("running it when {} failed: {}", given(input, names), inner.message),
    };
    let mut detail = format!("input: {}", input.json);
    for note in &inner.notes {
        detail.push_str("; ");
        detail.push_str(note);
    }
    Rejection {
        cause: Cause { code, name },
        input: given(input, names),
        verbose: format!("{}: {line} (input {})", Code::VerificationFailed.as_str(), input.json),
        line,
        reply: String::new(),
        diagnostics: vec![AttemptDiagnostic {
            code: Code::VerificationFailed.as_str().to_owned(),
            message: format!("The candidate failed verification: {}", inner.message),
            path: None,
            detail: Some(detail),
        }],
    }
}
