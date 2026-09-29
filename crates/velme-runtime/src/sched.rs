//! The wave scheduler (`runtime/30` §2 Scheduler, §4, §5): runs a goal's calls wave by wave on Tokio, each wave's calls
//! at the same time, and applies D-9 when one fails. Results and failures are in source order, never in the order
//! things finished, so a run is the same for every `--jobs` (R-RUN-07, R-RUN-12). Every invocation leaves a
//! [`GoalRun`], the record a trace is made from (§8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::runtime::Builder;
use tokio::task::JoinSet;
use velme_builtins::Value;
use velme_builtins::limits::MAX_WALL_CLOCK_MS;
use velme_check::Part;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_interp::{Budget, Error, Failure, Interrupt, Spent};
use velme_ir::Fingerprint;
use velme_sema::hir::{GoalId, GoalKind, Program};

use crate::clock::{Clock, SystemClock, Watchdog};
use crate::leaf::{Body, Progress, limits, run_body};
use crate::plan::waves;
use crate::registry::Registry;

/// Stack of each thread that evaluates a goal, as the CLI's is: the limits bound how deep evaluation and value
/// rendering recurse, and a blocking-pool thread's default stack is smaller than that needs on some platforms.
const EVAL_STACK: usize = 64 * 1024 * 1024;

/// How a run is scheduled and watched.
#[derive(Debug, Clone)]
pub struct Options {
    /// The most goal bodies evaluated at once (`--jobs`, R-RUN-07); at least 1. It changes when things run, never what
    /// they produce.
    pub jobs: usize,
    /// The watchdog's clock, injectable so tests need no real time (`runtime/30` R-RUN-24, R-QA-02).
    pub clock: Arc<dyn Clock>,
    /// How long the whole run may take by that clock before the watchdog stops it with `VL0603`: `max_wall_clock`, 60 s
    /// (D-51). Only a safety net: the deterministic limits stop a run long before.
    pub max_wall_clock: Duration,
}

impl Default for Options {
    /// One job per available CPU, the host's clock and the system `max_wall_clock`.
    fn default() -> Self {
        Options {
            jobs: std::thread::available_parallelism().map_or(1, usize::from),
            clock: Arc::new(SystemClock::new()),
            max_wall_clock: Duration::from_millis(MAX_WALL_CLOCK_MS),
        }
    }
}

impl Options {
    /// The same options with `jobs` workers.
    pub fn with_jobs(mut self, jobs: usize) -> Options {
        self.jobs = jobs;
        self
    }
}

/// When a goal ran, as offsets from the start of the run. Timing differs from run to run, so it is excluded from
/// comparisons: any two timings are equal (`runtime/30` §8).
#[derive(Debug, Clone, Copy, Default)]
pub struct Timing {
    /// When the goal started: a leaf's body started being evaluated, a goal with calls started running its first wave.
    /// A goal waiting for a worker hasn't started.
    pub start: Duration,
    /// When it finished.
    pub end: Duration,
}

impl Timing {
    /// Whether the two ran at the same time for a while.
    pub fn overlaps(&self, other: &Timing) -> bool {
        self.start < other.end && other.start < self.end
    }
}

impl PartialEq for Timing {
    fn eq(&self, _: &Timing) -> bool {
        true
    }
}

impl Eq for Timing {}

/// Why a goal failed: the chain of goals from the one that ran down to the one whose own work failed, and the failure of
/// that one (R-RUN-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// The goals from the one that failed, outermost first, to the one where it failed. Its length is 1 for a goal that
    /// failed in its own body or checks.
    path: Vec<String>,
    /// What the innermost goal reported: its failure, or every failed check in source order (R-CHK-09).
    diagnostics: Vec<Diagnostic>,
    /// The other calls that failed in the same wave, in source order (R-RUN-09).
    notes: Vec<String>,
}

impl Failed {
    fn new(goal: &str, diagnostics: Vec<Diagnostic>) -> Failed {
        Failed {
            path: vec![goal.to_owned()],
            diagnostics,
            notes: Vec::new(),
        }
    }

    fn internal(goal: &str) -> Failed {
        Failed::new(goal, vec![Diagnostic::internal_error()])
    }

    /// The diagnostics to report. A goal that failed itself reports its own; one that failed because a call did reports
    /// the root cause's, with its code, worded `A failed because B failed: …` and carrying the other failed calls of
    /// the wave as notes (R-RUN-09, R-RUN-10).
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        if self.path.len() < 2 {
            return self.diagnostics.clone();
        }
        // Every failed check of the root cause is kept, in source order (R-CHK-09); the notes go with the first.
        let mut wrapped: Vec<Diagnostic> = self
            .diagnostics
            .iter()
            .map(|d| {
                let mut d = d.clone();
                d.message = chain(&self.path, &d.message);
                d
            })
            .collect();
        if let Some(first) = wrapped.first_mut() {
            first.notes.extend(self.notes.iter().cloned());
        }
        wrapped
    }

    /// The goals from the one that ran down to the root cause: `BuildPlayerSummary › FindBadge` (`runtime/30` §8).
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// The code of the root cause, which decides the exit status (R-RUN-10).
    pub fn code(&self) -> Option<velme_diagnostics::Code> {
        self.diagnostics.first().map(|d| d.code)
    }

    /// The failure as notes of the goal `parent`, which it didn't stop: one line per diagnostic, `A failed because B
    /// failed: …` and its code, then the notes it carries of its own failed calls.
    fn as_notes(&self, parent: &str) -> Vec<String> {
        let mut notes: Vec<String> = self
            .diagnostics
            .iter()
            .map(|d| {
                format!(
                    "`{parent}` also failed because {} [{}]",
                    chain(&self.path, &d.message),
                    d.code.as_str()
                )
            })
            .collect();
        notes.extend(self.notes.iter().cloned());
        notes
    }
}

/// `` `A` failed because `B` failed: message ``.
fn chain(path: &[String], message: &str) -> String {
    let goals: Vec<String> = path.iter().map(|goal| format!("`{goal}`")).collect();
    format!("{} failed: {message}", goals.join(" failed because "))
}

/// What became of one `call` binding of a goal (`runtime/30` §8 `call`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRun {
    /// The bound name.
    pub binding: String,
    /// The goal called.
    pub callee: String,
    /// The wave it belongs to, from 1 (R-RUN-06).
    pub wave: usize,
    /// The arguments it was called with, one per parameter; empty if it never started.
    pub args: Vec<Value>,
    /// The callee's run; `None` if no later wave than a failed one starts, so this one never did (`skipped`, R-RUN-09).
    pub run: Option<GoalRun>,
}

/// The outcome of a call in a trace (`runtime/30` §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// It ran and produced a value.
    Ok,
    /// It ran and failed.
    Failed,
    /// It never started, because a wave before it failed (D-9).
    Skipped,
}

impl CallRun {
    /// How the call ended.
    pub fn status(&self) -> CallStatus {
        match &self.run {
            None => CallStatus::Skipped,
            Some(GoalRun { outcome: Ok(_), .. }) => CallStatus::Ok,
            Some(_) => CallStatus::Failed,
        }
    }
}

/// One check item of a goal's invocation (`runtime/30` §8 `check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    /// The item as written.
    pub text: String,
    /// Where it is in the source.
    pub span: Span,
    /// Whether it held.
    pub passed: bool,
    /// For a failed item, the value of every path and helper call in it, and the operands never evaluated
    /// (`language/13` R-CHK-10); empty for an item that held.
    pub values: Vec<Part>,
}

/// One invocation of a goal: what its calls did in source order, and how it ended. Nothing in it depends on how the run
/// was scheduled except the timings, which comparisons ignore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRun {
    /// The goal.
    pub goal: String,
    /// The hash of the locked artifact it ran; `None` if it never got that far.
    pub artifact: Option<Fingerprint>,
    /// What is synthesized for it.
    pub kind: GoalKind,
    /// Its inputs, by parameter name, in order.
    pub inputs: Vec<(String, Value)>,
    /// One record per `call` binding, in block order, whether or not it ran.
    pub calls: Vec<CallRun>,
    /// One record per check item in source order, if its checks ran.
    pub checks: Vec<CheckRun>,
    /// Fuel its body used; 0 if it never ran.
    pub fuel: u64,
    /// Memory its body used, in cumulative bytes allocated (`runtime/30` §7.1); 0 if it never ran.
    pub memory: u64,
    /// Its value, once its calls, body and checks have all succeeded; otherwise why not.
    pub outcome: Result<Value, Failed>,
    /// When it ran.
    pub timing: Timing,
}

impl GoalRun {
    /// A run of `goal` that has not got anywhere: an internal error until something else is known.
    fn unstarted(goal: &str, kind: GoalKind) -> GoalRun {
        GoalRun {
            goal: goal.to_owned(),
            artifact: None,
            kind,
            inputs: Vec::new(),
            calls: Vec::new(),
            checks: Vec::new(),
            fuel: 0,
            memory: 0,
            outcome: Err(Failed::internal(goal)),
            timing: Timing::default(),
        }
    }

    /// Whether the run would end the same way every time (`runtime/30` R-RUN-14, D-10): `false` if this invocation or any
    /// call under it was stopped by the wall-clock watchdog, `VL0603`, whatever else the run reports. Such an outcome
    /// is never cached and never a verification verdict.
    pub fn reproducible(&self) -> bool {
        let timed_out = self
            .outcome
            .as_ref()
            .err()
            .is_some_and(|failed| failed.diagnostics.iter().any(|d| d.code == Code::Timeout));
        !timed_out
            && self
                .calls
                .iter()
                .all(|call| call.run.as_ref().is_none_or(GoalRun::reproducible))
    }

    /// The value, or the diagnostics of the failure.
    pub fn result(&self) -> Result<Value, Vec<Diagnostic>> {
        match &self.outcome {
            Ok(value) => Ok(value.clone()),
            Err(failed) => Err(failed.diagnostics()),
        }
    }
}

/// What every invocation of a run shares.
struct Shared {
    program: Program,
    source: String,
    registry: Registry,
    origin: Instant,
    /// The run's watchdog (once out of time, always out, so every invocation stops).
    watchdog: Arc<Watchdog>,
    /// How many goal bodies are being evaluated now, and the most there have been at once.
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

/// Runs the goal `goal` of `program`, whose source is `source`, on `inputs`, with every locked artifact it needs in
/// `registry` (`runtime/30` §4): its calls wave by wave, then its body, then its checks. Bodies are evaluated on at most
/// `options.jobs` threads at once.
pub fn run_goal(
    program: &Program,
    goal: GoalId,
    source: &str,
    registry: &Registry,
    inputs: Vec<Value>,
    options: Options,
) -> GoalRun {
    run_goal_peak(program, goal, source, registry, inputs, options).0
}

/// [`run_goal`], and the most goal bodies that were being evaluated at the same time: at most `options.jobs`, and more
/// than 1 only if bodies really ran concurrently. It differs with `--jobs`, so it is not part of the run.
pub fn run_goal_peak(
    program: &Program,
    goal: GoalId,
    source: &str,
    registry: &Registry,
    inputs: Vec<Value>,
    options: Options,
) -> (GoalRun, usize) {
    let name = program.goals.get(goal.0).map_or("", |g| g.name.as_str());
    let runtime = Builder::new_current_thread()
        .max_blocking_threads(options.jobs.max(1))
        .thread_stack_size(EVAL_STACK)
        .build();
    let Ok(runtime) = runtime else {
        let kind = program.goals.get(goal.0).map_or(GoalKind::Leaf, |g| g.kind);
        return (GoalRun::unstarted(name, kind), 0);
    };
    let shared = Arc::new(Shared {
        program: program.clone(),
        source: source.to_owned(),
        registry: registry.clone(),
        origin: Instant::now(),
        watchdog: Arc::new(Watchdog::start(&options)),
        in_flight: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    let run = runtime.block_on(invocation(Arc::clone(&shared), goal, inputs));
    (run, shared.peak.load(Ordering::SeqCst))
}

impl Shared {
    /// Whether the run has been going longer than `max_wall_clock` by its clock (`runtime/30` R-RUN-24, D-10).
    fn timed_out(&self) -> bool {
        self.watchdog.timed_out()
    }
}

/// The watchdog as the interpreter sees it: asked every so often, it says whether the run is out of time.
fn watchdog(shared: &Shared) -> Interrupt {
    shared.watchdog.interrupt()
}

/// `run`, which started at `started`, ending now with `failed`.
fn fail(shared: &Shared, mut run: GoalRun, started: Duration, failed: Failed) -> GoalRun {
    run.outcome = Err(failed);
    run.timing = Timing {
        start: started,
        end: shared.origin.elapsed(),
    };
    run
}

/// `run`, its goal declared at `span` and started at `started`, stopped by the watchdog before it finished.
fn stop_timed_out(shared: &Shared, run: GoalRun, span: Span, started: Duration) -> GoalRun {
    let diagnostic = Failure::from(Error::Interrupted).diagnostic(&run.goal, span);
    let failed = Failed::new(&run.goal, vec![diagnostic]);
    fail(shared, run, started, failed)
}

/// One invocation, boxed because a goal's calls are invocations too.
fn invocation(shared: Arc<Shared>, goal: GoalId, inputs: Vec<Value>) -> Pin<Box<dyn Future<Output = GoalRun> + Send>> {
    Box::pin(invoke(shared, goal, inputs))
}

async fn invoke(shared: Arc<Shared>, goal: GoalId, inputs: Vec<Value>) -> GoalRun {
    let started = shared.origin.elapsed();
    let Some(target) = shared.program.goals.get(goal.0) else {
        return GoalRun::unstarted("", GoalKind::Leaf);
    };
    let name = target.name.as_str();
    let mut run = GoalRun {
        inputs: target
            .params
            .iter()
            .map(|param| param.name.clone())
            .zip(inputs.iter().cloned())
            .collect(),
        artifact: shared.registry.get(goal).map(|locked| locked.artifact),
        calls: target
            .bindings
            .iter()
            .map(|binding| CallRun {
                binding: binding.name.clone(),
                callee: shared
                    .program
                    .goals
                    .get(binding.callee.0)
                    .map_or_else(String::new, |g| g.name.clone()),
                wave: binding.wave,
                args: Vec::new(),
                run: None,
            })
            .collect(),
        ..GoalRun::unstarted(name, target.kind)
    };
    if shared.registry.get(goal).is_none() {
        return run;
    }
    let mut values: Vec<Option<Value>> = vec![None; target.bindings.len()];
    let limits = limits(target);
    // What this invocation has spent on its calls' arguments, which its body spends after (R-RUN-17).
    let mut spent = Spent::default();
    for wave in waves(target) {
        if shared.timed_out() {
            return stop_timed_out(&shared, run, target.span, started);
        }
        // Arguments are evaluated first, so a wave starts only if all of it can; they are the parent's expressions, so
        // they run on the pool with its bodies and spend the parent's fuel.
        let evaluated = {
            let shared = Arc::clone(&shared);
            let (inputs, values, wave) = (inputs.clone(), values.clone(), wave.clone());
            let interrupt = watchdog(&shared);
            tokio::task::spawn_blocking(move || {
                let program = &shared.program;
                let (Some(target), Some(locked)) = (program.goals.get(goal.0), shared.registry.get(goal)) else {
                    return (Err(Diagnostic::internal_error()), spent);
                };
                let mut spent = spent;
                let mut all = Vec::new();
                for i in wave {
                    let budget = Budget::new(limits).after(spent).watched(Some(interrupt.clone()));
                    let (args, total) = velme_interp::call_args(&locked.ir, i, &inputs, &values, budget);
                    spent = total;
                    match args {
                        Ok(args) => all.push((i, args)),
                        Err(failure) => return (Err(failure.diagnostic(&target.name, target.span)), spent),
                    }
                }
                (Ok(all), spent)
            })
            .await
        };
        let (evaluated, total) = match evaluated {
            Ok(evaluated) => evaluated,
            Err(_) => return run,
        };
        // Spent whether the arguments evaluated or not: a failure's figures are what it spent before stopping.
        spent = total;
        (run.fuel, run.memory) = (spent.fuel, spent.memory);
        let evaluated = match evaluated {
            Ok(evaluated) => evaluated,
            Err(diagnostic) => return fail(&shared, run, started, Failed::new(name, vec![diagnostic])),
        };
        let mut starts = Vec::new();
        for (i, args) in evaluated {
            let Some(binding) = target.bindings.get(i) else {
                return run;
            };
            starts.push((i, binding.callee, args));
        }
        let mut children = JoinSet::new();
        for (i, callee, args) in starts {
            if let Some(call) = run.calls.get_mut(i) {
                call.args.clone_from(&args);
            }
            let shared = Arc::clone(&shared);
            children.spawn(async move { (i, invocation(shared, callee, args).await) });
        }
        // Every call of the wave runs to completion, whatever the others do (D-9); they finish in any order.
        let mut done = Vec::new();
        let mut lost = false;
        while let Some(joined) = children.join_next().await {
            match joined {
                Ok(child) => done.push(child),
                Err(_) => lost = true,
            }
        }
        done.sort_by_key(|(i, _)| *i);
        let mut failures = Vec::new();
        for (i, child) in done {
            match (&child.outcome, values.get_mut(i), run.calls.get_mut(i)) {
                (Ok(value), Some(slot), Some(call)) => {
                    *slot = Some(value.clone());
                    call.run = Some(child);
                }
                (Err(failed), _, Some(call)) => {
                    failures.push(failed.clone());
                    call.run = Some(child);
                }
                _ => return run,
            }
        }
        if lost {
            return run;
        }
        if let Some((first, others)) = failures.split_first() {
            let mut path = vec![name.to_owned()];
            path.extend(first.path.iter().cloned());
            let mut notes = first.notes.clone();
            for other in others {
                notes.extend(other.as_notes(name));
            }
            let failed = Failed {
                path,
                diagnostics: first.diagnostics.clone(),
                notes,
            };
            return fail(&shared, run, started, failed);
        }
    }
    let bindings: Vec<Value> = values.into_iter().flatten().collect();
    let has_calls = !bindings.is_empty();
    if shared.timed_out() {
        return stop_timed_out(&shared, run, target.span, started);
    }
    let body = {
        let progress = Progress {
            spent,
            interrupt: Some(watchdog(&shared)),
        };
        let shared = Arc::clone(&shared);
        tokio::task::spawn_blocking(move || {
            let now = shared.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            shared.peak.fetch_max(now, Ordering::SeqCst);
            let start = shared.origin.elapsed();
            let result = match shared.registry.get(goal) {
                Some(locked) => run_body(
                    &shared.program,
                    goal,
                    &shared.source,
                    locked,
                    inputs,
                    bindings,
                    progress,
                ),
                None => Body::internal(),
            };
            let end = shared.origin.elapsed();
            shared.in_flight.fetch_sub(1, Ordering::SeqCst);
            (result, start, end)
        })
        .await
    };
    match body {
        Ok((body, start, end)) => {
            run.fuel = body.fuel;
            run.memory = body.memory;
            run.checks = body.checks;
            run.outcome = body.result.map_err(|diagnostics| Failed::new(name, diagnostics));
            run.timing = Timing {
                start: if has_calls { started } else { start },
                end,
            };
        }
        Err(_) => run.timing = Timing::default(),
    }
    run
}
