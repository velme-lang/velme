//! `velme build` (`compiler/20` R-CMP-20, `runtime/32` R-ART-15, `compiler/22` R-SYNTH-02): every goal of a file gets a
//! verified artifact and a lock entry, in post-order, one goal at a time (R-SYNTH-01, D-93). The lookup order is the lock,
//! then the store by `synthesis_key` after the identity step, then the provider; a goal that needs no provider makes no
//! contact with one (D-92).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tokio::runtime::Builder;
use velme_builtins::Value;
use velme_check::Invocation;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::{
    CallNode, Fingerprint, Origin, Request, Synthesis, ValidIr, calls, contract_key, signature, synthesis_key,
    to_canonical_string, validate, wired_goal,
};
use velme_sema::hir::{GoalId, GoalKind, Program};
use velme_synth::{
    AttemptFeedback, ChildRunner, Identity, Outcome, Rejection, Session, SynthBackend, SynthOptions, SynthProvider,
    Task, Usage, Verdict, Verified, provider_diagnostic, reaches_no_further, synthesize, verify,
};

use crate::artifact::{ArtifactFormat, Child, Manifest, Verification};
use crate::lock::{Entry, Lock};
use crate::locked::{LockedGoal, load};
use crate::registry::Registry;
use crate::sched::{Options, run_goal_unchecked};
use crate::store::{Store, StoreError};

/// The compiler's version, recorded in every manifest (`runtime/32` R-ART-04).
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The provider id of an artifact the compiler wrote (`runtime/32` R-ART-07).
const COMPILER: &str = "compiler";

/// The provider id of an external backend, whose identity step can fail before its own name is known.
const EXTERNAL: &str = "external";

/// What a manifest records of who wrote the artifact (`runtime/32` R-ART-07, R-ART-21).
struct Provenance<'a> {
    provider: &'a str,
    prompt_version: Option<String>,
    model_version: Option<String>,
    backend: Option<String>,
}

impl Provenance<'_> {
    /// An artifact the compiler wrote.
    const COMPILER: Provenance<'static> = Provenance {
        provider: COMPILER,
        prompt_version: None,
        model_version: None,
        backend: None,
    };

    /// An artifact a provider wrote, under `identity`.
    fn of(identity: &Identity) -> Provenance<'_> {
        Provenance {
            provider: &identity.provider,
            prompt_version: Some(identity.input_version.clone()),
            model_version: Some(identity.model.clone()),
            backend: identity.backend.clone(),
        }
    }
}

/// What a build works on.
pub struct BuildInput<'a> {
    /// The checked program.
    pub program: &'a Program,
    /// Its source text.
    pub source: &'a str,
    /// The project root, where `velme.lock` and `.velme/` live.
    pub project: &'a Path,
    /// The file's path from the project root, as the lock names it (`tooling/40` R-CLI-19).
    pub file: &'a str,
    /// The provider, or `None` where synthesis is impossible by construction (`compiler/20` R-CMP-19).
    pub backend: Option<&'a dyn SynthBackend>,
    /// The `[synthesis]` settings.
    pub options: SynthOptions,
    /// How verification runs are scheduled and watched.
    pub run: Options,
}

/// Where a goal's artifact came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The lock entry was fresh (`runtime/32` R-ART-14).
    Lock,
    /// The lock entry was fresh and the artifact passed its examples and checks again against a changed child (R-ART-22).
    Reverified,
    /// The store held an artifact built under the same `synthesis_key`.
    Store,
    /// The provider wrote it and it passed verification.
    Synthesized,
    /// The compiler wrote it: a wired goal (D-4).
    Compiler,
}

/// How one goal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// It has an artifact, from `Source`.
    Built(Source),
    /// It has none: `VL04xx` other than the two below, `VL0503`, `VL0603`, `VL0607`, `VL0901`.
    Failed,
    /// `VL0408`: queued for a person or tool.
    Pending,
    /// `VL0409`: a goal it calls has none.
    Blocked,
}

/// One goal of the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalOutcome {
    /// The goal's name.
    pub goal: String,
    /// How it ended.
    pub status: Status,
    /// Why it has no artifact.
    pub diagnostics: Vec<Diagnostic>,
    /// What the learner should also know: a checked-again failure (R-SYNTH-46).
    pub notes: Vec<String>,
    /// One line per failed attempt, for `--verbose` (R-SYNTH-13).
    pub attempts: Vec<String>,
}

/// The build summary (`compiler/22` R-SYNTH-21).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Provider calls made.
    pub calls: usize,
    /// Tokens used.
    pub usage: Usage,
    /// Goals whose lock entry was fresh.
    pub lock_hits: usize,
    /// Goals taken from the store.
    pub store_hits: usize,
    /// Goals the provider wrote.
    pub synthesized: usize,
}

/// What a build did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    /// One per goal, in the order they were built.
    pub goals: Vec<GoalOutcome>,
    /// Problems that belong to no goal: an unreadable or unwritable lock.
    pub diagnostics: Vec<Diagnostic>,
    /// The summary.
    pub summary: Summary,
}

/// Builds every goal of `input.program` that has no fresh lock entry and writes the store and the lock. `first_contact`
/// is called once, before anything reaches a provider, the identity step included, so the caller can print what is sent
/// (`tooling/41` R-SEC-12); a build that contacts none never calls it.
pub fn build(input: &BuildInput<'_>, first_contact: &mut dyn FnMut()) -> BuildReport {
    let failed = |diagnostic| BuildReport {
        goals: Vec::new(),
        diagnostics: vec![diagnostic],
        summary: Summary::default(),
    };
    let runtime = match Builder::new_current_thread().build() {
        Ok(runtime) => runtime,
        Err(_) => return failed(Diagnostic::internal_error()),
    };
    let lock = match Lock::read(input.project) {
        Ok(lock) => lock.unwrap_or_else(|| Lock::new(input.program.language_version.clone())),
        Err(error) => return failed(error.diagnostic()),
    };
    runtime.block_on(Build::new(input, lock).run(first_contact))
}

/// The state of one build.
struct Build<'a> {
    input: &'a BuildInput<'a>,
    store: Store,
    lock: Lock,
    session: Session,
    /// The goals with an artifact, loaded or built.
    built: BTreeMap<GoalId, LockedGoal>,
    /// The goals with none, and the code each ended with (R-SYNTH-42).
    failed: BTreeMap<GoalId, Code>,
    /// The goals whose artifact changed, or that were checked again against a child that did (R-ART-22).
    touched: BTreeSet<GoalId>,
    identity: Option<Result<Identity, velme_synth::ProviderError>>,
    provider: Option<Box<dyn SynthProvider>>,
    summary: Summary,
}

impl<'a> Build<'a> {
    fn new(input: &'a BuildInput<'a>, lock: Lock) -> Self {
        Build {
            input,
            store: Store::new(input.project),
            lock,
            session: Session::new(input.options.clone()),
            built: BTreeMap::new(),
            failed: BTreeMap::new(),
            touched: BTreeSet::new(),
            identity: None,
            provider: None,
            summary: Summary::default(),
        }
    }

    async fn run(mut self, first_contact: &mut dyn FnMut()) -> BuildReport {
        let before = self.lock.clone();
        let mut goals = Vec::new();
        for id in post_order(self.input.program) {
            let outcome = self.goal(id, first_contact).await;
            if let Some(code) = outcome.diagnostics.first().map(|d| d.code)
                && !matches!(outcome.status, Status::Built(_))
            {
                self.failed.insert(id, code);
            }
            goals.push(outcome);
        }
        let mut diagnostics = Vec::new();
        let names: Vec<&str> = self.input.program.goals.iter().map(|g| g.name.as_str()).collect();
        self.lock.retain_declared(self.input.file, &names);
        self.lock.language.clone_from(&self.input.program.language_version);
        if self.lock != before
            && let Err(error) = self.lock.write(self.input.project)
        {
            diagnostics.push(Diagnostic::new(
                Code::FileError,
                Span::default(),
                format!("I couldn't write `velme.lock`: {error}."),
            ));
        }
        self.summary.calls = self.session.calls();
        self.summary.usage = self.session.usage();
        BuildReport {
            goals,
            diagnostics,
            summary: self.summary,
        }
    }

    async fn goal(&mut self, id: GoalId, first_contact: &mut dyn FnMut()) -> GoalOutcome {
        let program = self.input.program;
        let Some(target) = program.goals.get(id.0) else {
            return outcome("", Status::Failed, vec![Diagnostic::internal_error()]);
        };
        let (name, span) = (target.name.clone(), target.span);
        if let Some((child, code)) = target
            .bindings
            .iter()
            .find_map(|b| self.failed.get(&b.callee).map(|code| (b.callee, *code)))
        {
            let child_name = program.goals.get(child.0).map_or("", |g| g.name.as_str());
            let d = Diagnostic::new(
                Code::SynthesisBlocked,
                span,
                format!("`{name}` wasn't built because `{child_name}` couldn't be built."),
            )
            .with_note(format!("`{child_name}` ended with {}", code.as_str()));
            return outcome(&name, Status::Blocked, vec![d]);
        }
        let mut notes = Vec::new();
        let mut feedback = Vec::new();
        if let Ok(locked) = load(program, id, self.input.file, &self.lock, &self.store) {
            // The manifest doesn't record the artifacts of its children, so a goal with calls is checked again against
            // the current ones on every build, on the interpreter and with no provider (R-ART-22): a build that failed
            // to rebuild it after a child changed leaves the lock as it was, and this finds it stale next time.
            if target.bindings.is_empty() {
                self.summary.lock_hits += 1;
                self.built.insert(id, locked);
                return done(&name, Status::Built(Source::Lock), notes);
            }
            let candidate = locked.ir.clone();
            match self.verify(id, &candidate) {
                Verdict::Accepted(_) => {
                    self.summary.lock_hits += 1;
                    // Only a goal with a callee that changed is itself changed for the note of its own callers.
                    if target_calls(program, id).any(|c| self.touched.contains(&c)) {
                        self.touched.insert(id);
                    }
                    self.built.insert(id, locked);
                    return done(&name, Status::Built(Source::Reverified), notes);
                }
                Verdict::Rejected(rejection) => {
                    notes.push(self.recheck_note(id, &rejection.line));
                    feedback.push(previous(&candidate, &rejection));
                }
                Verdict::Watchdog(d) => return outcome(&name, Status::Failed, vec![retarget(d, span)]),
                Verdict::Internal => return outcome(&name, Status::Failed, vec![Diagnostic::internal_error()]),
            }
        }
        let mut result = if target.kind == GoalKind::Wired {
            self.wired(id, &name, span)
        } else {
            self.synthesize(id, &name, span, feedback, first_contact).await
        };
        notes.append(&mut result.notes);
        noted(result, notes)
    }

    /// The children of `id` that changed in this build, quoted and named (R-SYNTH-46).
    fn changed_children(&self, id: GoalId) -> Vec<String> {
        let program = self.input.program;
        target_calls(program, id)
            .filter(|c| self.touched.contains(c))
            .filter_map(|c| program.goals.get(c.0).map(|g| format!("`{}`", g.name)))
            .collect()
    }

    /// The note of a goal that was checked again and no longer passes, naming the children that changed (R-SYNTH-46).
    fn recheck_note(&self, id: GoalId, line: &str) -> String {
        let name = self.input.program.goals.get(id.0).map_or("", |g| g.name.as_str());
        let changed = self.changed_children(id);
        if changed.is_empty() {
            format!("`{name}` was checked again against the goals it calls, and no longer passes: {line}")
        } else {
            format!(
                "`{name}` was checked again because {} changed, and no longer passes: {line}",
                changed.join(" and ")
            )
        }
    }

    /// The note of a stored version of a goal that doesn't pass, naming the children that changed (R-SYNTH-46).
    fn stored_note(&self, id: GoalId, line: &str) -> String {
        let name = self.input.program.goals.get(id.0).map_or("", |g| g.name.as_str());
        let changed = self.changed_children(id);
        if changed.is_empty() {
            format!("a stored version of `{name}` didn't pass: {line}")
        } else {
            format!(
                "a stored version of `{name}` didn't pass after {} changed: {line}",
                changed.join(" and ")
            )
        }
    }

    /// Verifies `candidate` for `id` against the children built so far.
    fn verify(&self, id: GoalId, candidate: &ValidIr) -> Verdict {
        let Ok(contract) = contract_key(self.input.program, id) else {
            return Verdict::Internal;
        };
        let runner = Runner { build: self, goal: id };
        verify(self.input.program, id, self.input.source, contract, candidate, &runner)
    }

    /// A wired goal: the compiler's IR, verified like any candidate, needing no provider (D-4, AC-CMP-06).
    fn wired(&mut self, id: GoalId, name: &str, span: Span) -> GoalOutcome {
        let program = self.input.program;
        let internal = || outcome(name, Status::Failed, vec![Diagnostic::internal_error()]);
        let (Ok(goal), Ok(compiler_calls)) = (wired_goal(program, id), calls(program, id)) else {
            return internal();
        };
        let Ok(text) = to_canonical_string(&goal) else {
            return internal();
        };
        let request = Request {
            program,
            goal: id,
            calls: &compiler_calls,
            origin: Origin::Complete,
        };
        let Ok(ir) = validate(&text, &request) else {
            return internal();
        };
        let verified = match self.verify(id, &ir) {
            Verdict::Accepted(verified) => verified,
            Verdict::Rejected(rejection) => {
                let check = if rejection.cause.name.is_empty() {
                    "its run"
                } else {
                    rejection.cause.name.as_str()
                };
                let d = Diagnostic::new(
                    Code::VerificationFailed,
                    span,
                    format!("The generated program didn't pass `{check}` for {}.", rejection.input),
                )
                .with_help("check the goal's examples and checks against what its calls give");
                return outcome(name, Status::Failed, vec![d]);
            }
            Verdict::Watchdog(d) => return outcome(name, Status::Failed, vec![retarget(d, span)]),
            Verdict::Internal => return internal(),
        };
        let synthesis = Synthesis {
            input_version: COMPILER,
            compiler_version: COMPILER_VERSION,
            provider: COMPILER,
            model: "",
        };
        match self.store_artifact(id, name, span, ir, verified, &synthesis, Provenance::COMPILER) {
            Ok(()) => done(name, Status::Built(Source::Compiler), Vec::new()),
            Err(d) => outcome(name, Status::Failed, vec![d]),
        }
    }

    async fn synthesize(
        &mut self,
        id: GoalId,
        name: &str,
        span: Span,
        mut feedback: Vec<AttemptFeedback>,
        first_contact: &mut dyn FnMut(),
    ) -> GoalOutcome {
        let mut notes: Vec<String> = Vec::new();
        let fail = |d: Diagnostic| {
            let status = if d.code == Code::SynthesisPending {
                Status::Pending
            } else {
                Status::Failed
            };
            outcome(name, status, vec![d])
        };
        let program = self.input.program;
        let Some(backend) = self.input.backend else {
            let d = velme_synth::unavailable(name, span, Some("this build may not use a provider"));
            return fail(d);
        };
        if self.identity.is_none() {
            first_contact();
            let identity = backend.identify().await;
            if let Err(error) = &identity
                && reaches_no_further(error)
            {
                self.session.stop(error);
            }
            self.identity = Some(identity);
        }
        let identity = match &self.identity {
            Some(Ok(identity)) => identity.clone(),
            Some(Err(error)) => return fail(provider_diagnostic(error, EXTERNAL, name, span)),
            None => return fail(Diagnostic::internal_error()),
        };
        let Ok(contract) = contract_key(program, id) else {
            return fail(Diagnostic::internal_error());
        };
        let pinned = self.lock.entry(self.input.file, name).map(|e| e.artifact);
        let synthesis = Synthesis {
            input_version: &identity.input_version,
            compiler_version: COMPILER_VERSION,
            provider: &identity.provider,
            model: &identity.model,
        };
        let Ok(key) = synthesis_key(contract, &synthesis) else {
            return fail(Diagnostic::internal_error());
        };
        // Step 2 of the lookup: an artifact built under the same key, loaded like any other (R-ART-10).
        for artifact in self.store.find_by_synthesis_key(key) {
            let Ok(signature) = signature(program, id) else {
                continue;
            };
            let mut probe = Lock::new(program.language_version.clone());
            probe.insert(Entry {
                file: self.input.file.to_owned(),
                name: name.to_owned(),
                signature,
                contract_key: contract,
                artifact,
            });
            if let Ok(locked) = load(program, id, self.input.file, &probe, &self.store) {
                // A hit is verified like any candidate before it is pinned, whoever wrote it (D-46, T-12): the store
                // checks its hashes, not its behaviour. A hit already found to fail in this build (the lock's own
                // artifact) isn't tried again.
                if pinned == Some(artifact) && !feedback.is_empty() {
                    continue;
                }
                match self.verify(id, &locked.ir) {
                    Verdict::Accepted(_) => {}
                    Verdict::Rejected(rejection) => {
                        notes.push(self.stored_note(id, &rejection.line));
                        feedback.push(previous(&locked.ir, &rejection));
                        continue;
                    }
                    Verdict::Watchdog(d) => return fail(retarget(d, span)),
                    Verdict::Internal => return fail(Diagnostic::internal_error()),
                }
                self.pin(id, name, signature, contract, artifact, pinned);
                self.built.insert(id, locked);
                self.summary.store_hits += 1;
                // The stored versions that failed before this one are of no interest once one is taken.
                return done(name, Status::Built(Source::Store), Vec::new());
            }
        }
        // Step 3: after one `VL0404` the provider is contacted no more, so every goal that gets this far ends with it and
        // no request (R-SYNTH-45, D-93).
        if let Some(error) = self.session.stopped() {
            return noted(fail(velme_synth::stopped_diagnostic(error, name, span)), notes);
        }
        if self.provider.is_none() {
            match backend.open(&identity) {
                Ok(provider) => self.provider = Some(provider),
                Err(error) => return fail(provider_diagnostic(&error, &identity.provider, name, span)),
            }
        }
        let Some(provider) = self.provider.take() else {
            return fail(Diagnostic::internal_error());
        };
        let task = Task {
            program,
            goal: id,
            source: self.input.source,
            contract_key: contract,
            synthesis_key: key,
            feedback,
        };
        let mut session = std::mem::replace(&mut self.session, Session::new(self.input.options.clone()));
        let synthesized = {
            let runner = Runner { build: self, goal: id };
            synthesize(&mut session, provider.as_ref(), &task, &runner).await
        };
        self.session = session;
        self.provider = Some(provider);
        match synthesized {
            Outcome::Failed(failure) => {
                let mut result = noted(fail(failure.diagnostic), notes);
                result.attempts = failure.attempts;
                result
            }
            Outcome::Built(built) => {
                let stored = self.store_artifact(
                    id,
                    name,
                    span,
                    built.ir,
                    built.verified,
                    &synthesis,
                    Provenance::of(&identity),
                );
                match stored {
                    Ok(()) => {
                        self.summary.synthesized += 1;
                        done(name, Status::Built(Source::Synthesized), notes)
                    }
                    Err(d) => noted(fail(d), notes),
                }
            }
        }
    }

    /// Writes the artifact of a verified `ir` and pins it (R-ART-11); the store refusing it is `VL0607` (R-SYNTH-47).
    #[allow(clippy::too_many_arguments)]
    fn store_artifact(
        &mut self,
        id: GoalId,
        name: &str,
        span: Span,
        ir: ValidIr,
        verified: Verified,
        synthesis: &Synthesis<'_>,
        provenance: Provenance<'_>,
    ) -> Result<(), Diagnostic> {
        let program = self.input.program;
        let (Ok(contract), Ok(sig)) = (contract_key(program, id), signature(program, id)) else {
            return Err(Diagnostic::internal_error());
        };
        let key = synthesis_key(contract, synthesis)?;
        let mut manifest = manifest(program, id, &ir, key)?;
        manifest.provider = provenance.provider.to_owned();
        manifest.prompt_version = provenance.prompt_version;
        manifest.model_version = provenance.model_version;
        manifest.backend = provenance.backend;
        manifest.verification = Verification {
            examples: verified.examples,
            generated_inputs: verified.generated_inputs,
            input_set: verified.input_set,
            max_fuel_observed: verified.max_fuel_observed,
        };
        let artifact = match self.store.put(&manifest, &ir) {
            Ok(artifact) => artifact,
            Err(StoreError::Io(error)) => {
                return Err(Diagnostic::new(
                    Code::FileError,
                    span,
                    format!("I couldn't store the built version of `{name}`: {error}."),
                ));
            }
            Err(_) => return Err(Diagnostic::internal_error()),
        };
        let previous = self.lock.entry(self.input.file, name).map(|e| e.artifact);
        self.pin(id, name, sig, contract, artifact, previous);
        self.built.insert(id, LockedGoal { artifact, manifest, ir });
        Ok(())
    }

    /// Pins `artifact` for the goal, and marks it touched if that changes what the lock held (R-ART-22).
    fn pin(
        &mut self,
        id: GoalId,
        name: &str,
        signature: Fingerprint,
        contract_key: Fingerprint,
        artifact: Fingerprint,
        previous: Option<Fingerprint>,
    ) {
        if previous != Some(artifact) {
            self.touched.insert(id);
        }
        self.lock.insert(Entry {
            file: self.input.file.to_owned(),
            name: name.to_owned(),
            signature,
            contract_key,
            artifact,
        });
    }
}

/// The goals `id` calls.
fn target_calls(program: &Program, id: GoalId) -> impl Iterator<Item = GoalId> + '_ {
    program
        .goals
        .get(id.0)
        .into_iter()
        .flat_map(|g| g.bindings.iter().map(|b| b.callee))
}

fn outcome(goal: &str, status: Status, diagnostics: Vec<Diagnostic>) -> GoalOutcome {
    GoalOutcome {
        goal: goal.to_owned(),
        status,
        diagnostics,
        notes: Vec::new(),
        attempts: Vec::new(),
    }
}

fn done(goal: &str, status: Status, notes: Vec<String>) -> GoalOutcome {
    GoalOutcome {
        goal: goal.to_owned(),
        status,
        diagnostics: Vec::new(),
        notes,
        attempts: Vec::new(),
    }
}

/// `outcome` with `notes` as its notes.
fn noted(mut outcome: GoalOutcome, notes: Vec<String>) -> GoalOutcome {
    outcome.notes = notes;
    outcome
}

/// A watchdog failure, pointing at the goal.
fn retarget(mut d: Diagnostic, span: Span) -> Diagnostic {
    d.span = span;
    d
}

/// The earlier attempt a checked-again failure becomes in the first request (R-SYNTH-46): the previous IR, as a reply
/// would be written, and why it fails now.
fn previous(ir: &ValidIr, rejection: &Rejection) -> AttemptFeedback {
    let mut goal = ir.goal().clone();
    goal.calls.clear();
    AttemptFeedback {
        reply: to_canonical_string(&goal).unwrap_or_default(),
        diagnostics: rejection.diagnostics.clone(),
    }
}

/// The goals in a topological order: each after every goal it calls, always the ready goal with the lowest source index
/// next (`compiler/22` R-SYNTH-01, D-93).
fn post_order(program: &Program) -> Vec<GoalId> {
    let mut order: Vec<GoalId> = Vec::new();
    let mut done = vec![false; program.goals.len()];
    while order.len() < program.goals.len() {
        let ready = (0..program.goals.len()).find(|&i| {
            !done.get(i).copied().unwrap_or(true)
                && target_calls(program, GoalId(i)).all(|c| done.get(c.0).copied().unwrap_or(true))
        });
        // A call cycle is an error of the program, which never gets this far; nothing ready ends the walk.
        let Some(next) = ready else { break };
        if let Some(slot) = done.get_mut(next) {
            *slot = true;
        }
        order.push(GoalId(next));
    }
    order
}

/// The manifest of `ir` under `key`, before the provider and verification are filled in (`runtime/32` §3).
pub(crate) fn manifest(program: &Program, id: GoalId, ir: &ValidIr, key: Fingerprint) -> Result<Manifest, Diagnostic> {
    let target = program.goals.get(id.0).ok_or_else(Diagnostic::internal_error)?;
    let goal = ir.goal();
    let children = goal
        .calls
        .iter()
        .map(|CallNode::Call(call)| {
            Ok(Child {
                binding: call.binding.clone(),
                goal: call.goal.clone(),
                signature: call.goal_signature.parse().map_err(|_| Diagnostic::internal_error())?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(Manifest {
        format: ArtifactFormat,
        goal: goal.goal.clone(),
        kind: target.kind.into(),
        signature: signature(program, id)?,
        contract_key: contract_key(program, id)?,
        synthesis_key: key,
        language_version: program.language_version.clone(),
        compiler_version: COMPILER_VERSION.to_owned(),
        ir_version: goal.ir_version.clone(),
        builtins_version: goal.builtins_version.clone(),
        prompt_version: None,
        provider: COMPILER.to_owned(),
        backend: None,
        model_version: None,
        children,
        verification: Verification {
            examples: 0,
            generated_inputs: 0,
            input_set: Fingerprint::of_bytes(b""),
            max_fuel_observed: 0,
        },
    })
}

/// Runs a candidate of one goal through the scheduler with the goals built so far (`compiler/22` R-SYNTH-14, D-98).
struct Runner<'b, 'a> {
    build: &'b Build<'a>,
    goal: GoalId,
}

impl ChildRunner for Runner<'_, '_> {
    fn run(&self, candidate: &ValidIr, inputs: Vec<Value>) -> Result<Invocation, Vec<Diagnostic>> {
        let input = self.build.input;
        let internal = || vec![Diagnostic::internal_error()];
        let stand_in =
            manifest(input.program, self.goal, candidate, Fingerprint::of_bytes(b"candidate")).map_err(|d| vec![d])?;
        let mut goals = self.build.built.clone();
        goals.insert(
            self.goal,
            LockedGoal {
                artifact: Fingerprint::of_bytes(b"candidate"),
                manifest: stand_in,
                ir: candidate.clone(),
            },
        );
        let registry = Registry::of(goals);
        // The scheduler runs a runtime of its own, which can't start inside the build's.
        let run = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    run_goal_unchecked(
                        input.program,
                        self.goal,
                        input.source,
                        &registry,
                        inputs,
                        input.run.clone(),
                    )
                })
                .join()
        })
        .map_err(|_| internal())?;
        let result = run.result()?;
        let bindings = run
            .calls
            .iter()
            .map(|call| match call.run.as_ref().map(|r| &r.outcome) {
                Some(Ok(value)) => Ok(value.clone()),
                _ => Err(internal()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Invocation {
            inputs: run.inputs.iter().map(|(_, v)| v.clone()).collect(),
            bindings,
            result,
            fuel: run.fuel,
            memory: run.memory,
        })
    }
}
