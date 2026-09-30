//! The backend that evaluates a leaf goal's body (`runtime/31` R-SBX-02, R-SBX-17, D-117): the reference interpreter,
//! or the WASM module of its IR in the process's sandbox. Everything else an invocation evaluates — its checks, its
//! examples, its calls' arguments, a composite's tail — is the interpreter's, and so is build verification (D-80).
//! Both give the same value, failure, fuel and memory (INV-3, R-SBX-18), so nothing after this point knows which ran.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

use velme_builtins::Value;
use velme_diagnostics::{Diagnostic, Span};
use velme_interp::{Budget, Interrupt, Limits, Spent};
use velme_ir::ValidIr;
use velme_sema::hir::{Goal, GoalKind};
use velme_wasm::{Backstop, CacheDir, EmitError, LoadError, Program, Run, Sandbox, cache_off_note, emit};

/// The most leaf bodies running on WASM at once, whatever `--jobs` is: the scheduler's other bodies wait for a slot.
/// A module's memory is never freed during its run (R-SBX-03), so each running body can hold up to its `max_memory`
/// (at most 64 MiB) plus its inputs, data and scratch: four keep the process under about 300 MiB of module memory on
/// any machine, while a run of the examples still overlaps its independent calls. It changes when bodies run, never
/// what they give (R-RUN-07).
pub const MAX_WASM_RUNS: usize = 4;

/// What evaluates leaf goal bodies (`--backend`, R-SBX-02). The default is the interpreter, also after the M7 gate,
/// where only the CLI's default becomes `auto` (D-117, D-121).
#[derive(Debug, Clone, Default)]
pub enum Backend {
    /// Every body on the reference interpreter.
    #[default]
    Interp,
    /// Leaf bodies on WASM; a leaf the WASM backend can't run is `VL0607` (`VL0801` for an import it refuses), never
    /// a silent fallback.
    Wasm(Arc<Wasm>),
    /// Leaf bodies on WASM, and on the interpreter a leaf whose module fails before it starts: the emitter declines
    /// it, or the sandbox can't be made, or can't load or start it (D-121). A leaf whose module started is never run
    /// again: a backstop firing is a backend bug (D-115).
    Auto(Arc<Wasm>),
}

/// The WASM backend of a process: its one sandbox, made the first time a leaf runs on WASM, so a process that never
/// asks never starts Wasmtime or its epoch ticker; it compiles each module once a process. And the notes `--verbose`
/// prints.
pub struct Wasm {
    /// The directory of the disk cache, passed in: the user-level one from `velme-cli`, a temporary one or none in a
    /// test (R-SBX-20).
    cache: Option<CacheDir>,
    sandbox: OnceLock<Result<Sandbox, LoadError>>,
    /// How many bodies are running on WASM now, at most [`MAX_WASM_RUNS`].
    running: Mutex<usize>,
    freed: Condvar,
    /// Sorted and without repeats, so they don't depend on the order the scheduler ran things in.
    notes: Mutex<BTreeSet<String>>,
    /// The stage at which the tests make a module fail before it starts (D-121).
    #[cfg(test)]
    fail: Option<Stage>,
}

/// A failure the tests inject before a module starts, where no real one can be made to happen (D-121).
#[cfg(test)]
#[derive(Debug, Clone)]
enum Stage {
    /// Making the sandbox: its engine, its linker or its epoch ticker.
    Sandbox,
    /// Loading the module: validating, compiling or linking it.
    Load(LoadError),
    /// Starting it: its thread, its instance or its inputs.
    Start,
}

impl std::fmt::Debug for Wasm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wasm")
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

/// Why a leaf body did not run on WASM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unrun {
    /// The emitter has no code for the goal, and says what about it (R-SBX-16).
    Declined(&'static str),
    /// The sandbox could not be made, or refused or failed to load the module: `VL0801` or `VL0607`.
    Load(LoadError),
    /// The module was loaded but never started: its thread, its instance or its inputs failed in the host (D-121).
    Unstarted,
    /// A bug of the backend or of its caller: `VL0607`.
    Internal,
}

impl Unrun {
    /// The failure of goal `goal`, declared at `span`, that did not run under an explicit `wasm`.
    pub fn diagnostic(&self, goal: &str, span: Span) -> Diagnostic {
        match self {
            Unrun::Declined(why) => Diagnostic::internal_error().with_note(format!(
                "the WASM backend can't run `{goal}` yet: it has no code for {why}; run it without `--backend wasm`"
            )),
            Unrun::Load(error) => error.diagnostic(goal, span),
            Unrun::Unstarted | Unrun::Internal => Diagnostic::internal_error(),
        }
    }

    /// Why, for the `--verbose` note of a leaf that `auto` ran on the interpreter (D-121).
    fn why(&self) -> String {
        match self {
            Unrun::Declined(why) => format!("the WASM backend has no code for {why}"),
            Unrun::Load(LoadError::Denied { import }) => {
                format!("the WASM backend refused its module, which imports `{import}`")
            }
            Unrun::Load(LoadError::Internal(error)) => format!("the WASM backend could not load its module: {error}"),
            Unrun::Load(_) => "the WASM backend could not load its module".to_owned(),
            Unrun::Unstarted => "the WASM backend could not start its module".to_owned(),
            Unrun::Internal => "the WASM backend could not run it".to_owned(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A place among the [`MAX_WASM_RUNS`], given back when dropped.
struct Slot<'a>(&'a Wasm);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *lock(&self.0.running) -= 1;
        self.0.freed.notify_one();
    }
}

impl Wasm {
    /// A backend whose sandbox, once made, keeps compiled modules in `cache`, or only in memory for `None`
    /// (R-SBX-20). Nothing is started until a leaf runs.
    pub fn new(cache: Option<CacheDir>) -> Wasm {
        Wasm {
            cache,
            sandbox: OnceLock::new(),
            running: Mutex::new(0),
            freed: Condvar::new(),
            notes: Mutex::new(BTreeSet::new()),
            #[cfg(test)]
            fail: None,
        }
    }

    /// A backend for the project at `root` whose disk cache is `cache`, as [`Wasm::new`], but only if the directory
    /// is absolute, has no `.` or `..` component, and lies outside the project once both are resolved (R-SBX-13,
    /// T-11): otherwise the disk cache is off for the process, with a note for `--verbose` (R-SBX-20).
    pub fn for_project(cache: Option<PathBuf>, root: &Path) -> Wasm {
        let Some(dir) = cache else {
            return Wasm::new(None);
        };
        match CacheDir::new(dir.clone(), root) {
            Ok(dir) => Wasm::new(Some(dir)),
            Err(why) => {
                let wasm = Wasm::new(None);
                wasm.note(cache_off_note(&dir, why));
                wasm
            }
        }
    }

    /// A backend whose modules fail at `stage`, for the tests of D-121.
    #[cfg(test)]
    fn failing(stage: Stage) -> Wasm {
        Wasm {
            fail: Some(stage),
            ..Wasm::new(None)
        }
    }

    /// What `--verbose` says on stderr about how the backend went, in order and once each (R-SBX-12, R-SBX-20):
    /// never part of `--json` or of a trace.
    pub fn notes(&self) -> Vec<String> {
        lock(&self.notes).iter().cloned().collect()
    }

    fn note(&self, note: String) {
        lock(&self.notes).insert(note);
    }

    /// The leaf body `ir` run on WASM on `inputs`, within `limits`, stopped from outside by `interrupt`: what the
    /// interpreter would give (INV-3), with how the backstops fared; or why it did not start, which is before the
    /// module ran any of the goal (D-121). `limits` are within [`Limits::SYSTEM`], as a goal's always are: more is
    /// refused as a bug (D-120).
    pub fn run(&self, ir: &ValidIr, inputs: &[Value], limits: Limits, interrupt: &Interrupt) -> Result<Run, Unrun> {
        if limits.fuel > Limits::SYSTEM.fuel || limits.memory > Limits::SYSTEM.memory {
            return Err(Unrun::Internal);
        }
        let program = self.program(ir)?;
        let _slot = self.slot();
        let run = program.run(inputs, limits, interrupt);
        if !self.started(&run) {
            return Err(Unrun::Unstarted);
        }
        Ok(run)
    }

    /// The loaded program of `ir`: emitted each time, compiled once a process by the sandbox, and with this goal's
    /// own signature and data, whichever goal's module had the same bytes first.
    fn program(&self, ir: &ValidIr) -> Result<Program, Unrun> {
        let module = emit(ir).map_err(|error| match error {
            EmitError::Declined(why) => Unrun::Declined(why),
            _ => Unrun::Internal,
        })?;
        let sandbox = self
            .sandbox
            .get_or_init(|| self.sandbox())
            .as_ref()
            .map_err(|error| Unrun::Load(error.clone()))?;
        #[cfg(test)]
        if let Some(Stage::Load(error)) = &self.fail {
            return Err(Unrun::Load(error.clone()));
        }
        let loaded = sandbox.load(&module);
        // The disk cache can be refused at any read or write, and is off from then on (D-120).
        if let Some(note) = sandbox.cache_off() {
            self.note(note);
        }
        loaded.map_err(Unrun::Load)
    }

    /// The process's sandbox, made on first use.
    fn sandbox(&self) -> Result<Sandbox, LoadError> {
        #[cfg(test)]
        if let Some(Stage::Sandbox) = &self.fail {
            return Err(LoadError::Internal("the tests refused to make a sandbox".to_owned()));
        }
        Sandbox::new(self.cache.clone())
    }

    /// Whether `run` reached `velme_run` (D-121).
    fn started(&self, run: &Run) -> bool {
        #[cfg(test)]
        if let Some(Stage::Start) = &self.fail {
            return false;
        }
        run.started
    }

    /// Waits for a place among the [`MAX_WASM_RUNS`].
    fn slot(&self) -> Slot<'_> {
        let mut running = lock(&self.running);
        while *running >= MAX_WASM_RUNS {
            running = self.freed.wait(running).unwrap_or_else(PoisonError::into_inner);
        }
        *running += 1;
        Slot(self)
    }
}

/// The body of the leaf goal `goal`, whose locked IR is `ir`, evaluated on `inputs` within `limits` and watched by
/// `interrupt`, on `backend` (R-SBX-17); a goal with calls is the interpreter's, and has no bindings here. Its value or
/// the failure's full diagnostic, and the fuel and memory spent up to where it stopped: the same on every backend
/// (R-SBX-18).
pub fn eval_leaf(
    backend: &Backend,
    goal: &Goal,
    ir: &ValidIr,
    inputs: Vec<Value>,
    limits: Limits,
    interrupt: Option<Interrupt>,
) -> (Result<Value, Diagnostic>, Spent) {
    let (wasm, explicit) = match backend {
        Backend::Wasm(wasm) if goal.kind == GoalKind::Leaf => (wasm, true),
        Backend::Auto(wasm) if goal.kind == GoalKind::Leaf => (wasm, false),
        _ => return interpret(goal, ir, inputs, limits, interrupt),
    };
    let watchdog = interrupt.clone().unwrap_or_else(|| Interrupt::new(|| false));
    match wasm.run(ir, &inputs, limits, &watchdog) {
        Ok(run) => {
            if let Some(backstop) = run.backstop {
                let which = match backstop {
                    Backstop::Fuel => "its Wasmtime fuel ran out",
                    Backstop::Memory => "Wasmtime refused to grow its memory",
                    _ => "a backstop fired",
                };
                wasm.note(format!(
                    "`{}` was stopped on WASM by a backstop before its own limit: {which}, which is a bug in Velme's \
                     WASM backend",
                    goal.name
                ));
            }
            let result = run.result.map_err(|failure| failure.diagnostic(&goal.name, goal.span));
            (result, run.spent)
        }
        // Nothing of the goal ran on WASM, so the interpreter's run is the whole of it (D-121).
        Err(unrun) if !explicit => {
            wasm.note(format!("`{}` ran on the interpreter: {}", goal.name, unrun.why()));
            interpret(goal, ir, inputs, limits, interrupt)
        }
        Err(unrun) => (Err(unrun.diagnostic(&goal.name, goal.span)), Spent::default()),
    }
}

/// The body on the reference interpreter.
fn interpret(
    goal: &Goal,
    ir: &ValidIr,
    inputs: Vec<Value>,
    limits: Limits,
    interrupt: Option<Interrupt>,
) -> (Result<Value, Diagnostic>, Spent) {
    let budget = Budget::new(limits).watched(interrupt);
    let (value, spent) = velme_interp::run_measured(ir, inputs, Vec::new(), budget);
    (
        value.map_err(|failure| failure.diagnostic(&goal.name, goal.span)),
        spent,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use velme_builtins::{BUILTINS_VERSION, Number};
    use velme_diagnostics::Code;
    use velme_ir::IR_VERSION;
    use velme_sema::hir::Program as Checked;
    use velme_test_support::{goal_id, program, valid_ir};

    const SOURCE: &str = "language: velme/0.1\n\ngoal Half(x: Number) -> Number:\n    plan: \"Halve x.\"\n";

    /// `Half`, which divides `x` by 2, and its IR.
    fn half() -> (Checked, ValidIr) {
        let program = program(SOURCE);
        let ir = format!(
            r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Half", "types": {{}},
                "inputs": [["x", {{"t": "Number"}}]], "output": {{"t": "Number"}}, "calls": [],
                "body": {{"kind": "binary", "op": "div", "left": {{"kind": "input", "name": "x"}},
                          "right": {{"kind": "literal", "type": {{"t": "Number"}}, "value": 2}}}}}}"#
        );
        let ir = valid_ir(&program, &ir);
        (program, ir)
    }

    fn on(backend: &Backend, x: i64) -> (Result<Value, Diagnostic>, Spent) {
        let (program, ir) = half();
        let goal = &program.goals[goal_id(&program, "Half").0];
        eval_leaf(
            backend,
            goal,
            &ir,
            vec![Value::from(Number::from(x))],
            Limits::SYSTEM,
            None,
        )
    }

    /// Under `auto` a leaf whose module fails before it starts — the sandbox, its load or its start — runs on the
    /// interpreter with a `--verbose` note saying why; under `wasm` each is reported: `VL0607`, or `VL0801` for an
    /// import the sandbox refuses (D-121, R-SBX-02).
    #[test]
    fn d_121_auto_runs_a_leaf_that_did_not_start_on_the_interpreter_and_wasm_reports_it() {
        let interp = on(&Backend::Interp, 7);
        assert_eq!(interp.0, Ok(Value::from(Number::parse("3.5").expect("a number"))));
        let denied = LoadError::Denied {
            import: "wasi.fd_write".to_owned(),
        };
        let stages = [
            (
                Stage::Sandbox,
                Code::InternalError,
                "the WASM backend could not load its module: the tests refused to make a sandbox",
            ),
            (
                Stage::Load(LoadError::Internal("it does not compile".to_owned())),
                Code::InternalError,
                "the WASM backend could not load its module: it does not compile",
            ),
            (
                Stage::Load(denied),
                Code::CapabilityDenied,
                "the WASM backend refused its module, which imports `wasi.fd_write`",
            ),
            (
                Stage::Start,
                Code::InternalError,
                "the WASM backend could not start its module",
            ),
        ];
        for (stage, code, why) in stages {
            let wasm = Arc::new(Wasm::failing(stage.clone()));
            let explicit = on(&Backend::Wasm(Arc::clone(&wasm)), 7);
            assert_eq!(explicit.0.expect_err("reported").code, code, "{stage:?}");
            assert_eq!(explicit.1, Spent::default(), "{stage:?}");
            assert!(wasm.notes().is_empty(), "{stage:?}");
            assert_eq!(on(&Backend::Auto(Arc::clone(&wasm)), 7), interp, "{stage:?}");
            assert_eq!(
                wasm.notes(),
                [format!("`Half` ran on the interpreter: {why}")],
                "{stage:?}"
            );
        }
    }

    /// A leaf whose module started is the module's outcome under `auto` too, a failure included: never run again on
    /// the interpreter (D-121, D-115).
    #[test]
    fn d_121_a_leaf_that_started_on_wasm_is_never_run_again() {
        let wasm = Arc::new(Wasm::new(None));
        let auto = Backend::Auto(Arc::clone(&wasm));
        assert_eq!(on(&auto, 7), on(&Backend::Interp, 7));
        let (program, ir) = half();
        let goal = &program.goals[goal_id(&program, "Half").0];
        // Out of fuel on WASM, which the interpreter would be too: reported as the module's, with no note.
        let short = Limits {
            fuel: 1,
            ..Limits::SYSTEM
        };
        let stopped = eval_leaf(&auto, goal, &ir, vec![Value::from(Number::from(7_i64))], short, None);
        assert_eq!(stopped.0.expect_err("out of fuel").code, Code::BudgetExceeded);
        assert!(wasm.notes().is_empty(), "{:?}", wasm.notes());
    }

    /// The disk cache is refused, with a note, when its directory is relative, has a `.` or `..` component, or lies
    /// inside the project once resolved: through a symbolic link to the project, or one to nowhere that making the
    /// directory would follow (T-11, R-SBX-13).
    #[test]
    fn t_11_a_cache_directory_that_is_relative_or_inside_the_project_is_refused() {
        let temp = std::env::temp_dir().join(format!("velme-backend-cache-{}", std::process::id()));
        let project = temp.join("project");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&project).expect("a project directory");
        let refused = |cache: PathBuf, why: &str| {
            let wasm = Wasm::for_project(Some(cache.clone()), &project);
            assert_eq!(wasm.cache, None, "{}", cache.display());
            assert_eq!(wasm.notes().len(), 1);
            assert!(wasm.notes()[0].contains(why), "{:?}", wasm.notes());
        };
        refused(PathBuf::from("velme/wasm"), "it is not an absolute path");
        refused(project.join(".cache/velme/wasm"), "it is inside the project");
        refused(
            temp.join("elsewhere/../project/velme/wasm"),
            "it has a `.` or `..` component",
        );
        refused(temp.join("cache/./velme/wasm"), "it has a `.` or `..` component");
        refused(temp.join("cache/velme/wasm/."), "it has a `.` or `..` component");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&project, temp.join("alias")).expect("a link");
            refused(temp.join("alias/velme/wasm"), "it is inside the project");
            // `..` after a directory that is not there yet: read as text it would skip to `alias`'s parent and look
            // outside, while the directories made would lead through `alias` into the project.
            refused(temp.join("nope/../alias/velme/wasm"), "it has a `.` or `..` component");
            assert!(!temp.join("nope").exists());
            std::os::unix::fs::symlink(project.join("missing"), temp.join("dangling")).expect("a link");
            refused(temp.join("dangling/velme/wasm"), "could not be resolved");
        }
        let outside = temp.join("cache/velme/wasm");
        let wasm = Wasm::for_project(Some(outside.clone()), &project);
        assert_eq!(wasm.cache.as_ref().map(CacheDir::path), Some(outside.as_path()));
        assert!(wasm.notes().is_empty(), "{:?}", wasm.notes());
        assert_eq!(Wasm::for_project(None, &project).cache, None);
        assert_eq!(std::fs::read_dir(&project).expect("the project").count(), 0);
        std::fs::remove_dir_all(&temp).expect("cleaned up");
    }
}
