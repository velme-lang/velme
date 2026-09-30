//! The Wasmtime sandbox (`runtime/31` §5-§7): an emitted module compiled, linked to the whitelist and nothing else,
//! and run once per invocation in a fresh store under the run's limits. The module's own meters decide a run
//! (R-SBX-05); Wasmtime's fuel, its resource limiter and the epoch watchdog stand behind them (R-SBX-12, INV-5).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use velme_builtins::execution::{Error, Failure, Interrupt, Limits, Spent};
use velme_builtins::limits::{MAX_GOAL_CALLS, MAX_LIST_SIZE, MAX_MEMORY, MIB};
use velme_builtins::{Function, Number, TEXT_BLOCK_BYTES, Value, range_length};
use velme_diagnostics::{Code, Diagnostic, Span};
use wasmparser::{Parser, Payload};
use wasmtime::{
    Caller, Config, Engine, Extern, Global, Instance, InstancePre, Linker, Memory, ResourceLimiter, Store, Trap,
    TypedFunc, UpdateDeadline, Val, WasmFeatures,
};

use crate::abi::{self, FRAME_BYTES, FRAME_MARSHAL, FRAME_VISITING, FRAMES, Import, MARSHAL_BYTES, Reason};
use crate::cache::{Cache, CacheDir};
use crate::code::PAGE_BYTES;
use crate::codec::{Image, Reader};
use crate::ty::Signature;
use crate::validate::{FEATURES, validate};
use crate::{Literals, Module, abi::SCRATCH_BYTES};

/// Instructions one byte of a text costs at most in the two functions that walk one, `text_eq` and `text_chars`:
/// the length of the longer body.
pub(crate) const BYTE_INSTRUCTIONS: u64 = 48;

/// The most instructions of a module that one paid Velme fuel unit covers (R-SBX-05, D-119): a block of
/// [`TEXT_BLOCK_BYTES`] bytes of a text, walked a byte at a time. Every other unit pays for one node, one element or
/// one comparison, each a bounded stretch of code.
pub const UNIT_INSTRUCTIONS: u64 = TEXT_BLOCK_BYTES * BYTE_INSTRUCTIONS;

/// K of the Wasmtime fuel backstop (`runtime/31` §6, D-115): 4 × [`UNIT_INSTRUCTIONS`].
pub const FUEL_FACTOR: u64 = 4 * UNIT_INSTRUCTIONS;

/// Instructions one item costs at most in `sum`, the one function paid after its work: the length of its body.
pub(crate) const SUM_ITEM_INSTRUCTIONS: u64 = 48;

/// Instructions that run unpaid besides: `velme_alloc` for the inputs, and storing the result.
const FIXED_INSTRUCTIONS: u64 = 1024;

/// A of the Wasmtime fuel backstop (`runtime/31` §6, D-115): 4 × what runs unpaid.
pub const FUEL_ALLOWANCE: u64 = 4 * (MAX_LIST_SIZE * SUM_ITEM_INSTRUCTIONS + FIXED_INSTRUCTIONS);

/// The most times emitted code moves one byte of a charged value with a bulk-memory copy: a `map` result into its
/// scratch frame and from there into its list (R-SBX-19). Every other copy moves a byte once, into the value that
/// is charged for it.
pub(crate) const MEMORY_MOVES: u64 = 2;

/// M of the Wasmtime fuel backstop (`runtime/31` §6, D-120): 4 × the most times emitted code moves one charged
/// byte (2), per byte of the run's memory, since Wasmtime charges a unit of its fuel per byte bulk memory moves.
pub const MEMORY_FACTOR: u64 = 4 * MEMORY_MOVES;

/// The Wasmtime fuel a run with `max_fuel` Velme fuel and a memory of at most `memory_limit` bytes gets:
/// `max_fuel × K + A + M × memory_limit`, saturating. A legal program never reaches it (D-115, D-120).
pub fn backstop_fuel(max_fuel: u64, memory_limit: u64) -> u64 {
    max_fuel
        .saturating_mul(FUEL_FACTOR)
        .saturating_add(FUEL_ALLOWANCE)
        .saturating_add(memory_limit.saturating_mul(MEMORY_FACTOR))
}

/// The stack a module may use (`runtime/31` §6, D-113).
const WASM_STACK_BYTES: usize = 4 * MIB as usize;

/// The stack of the thread a module runs on: the module's, and room for the host under and inside it, so a module
/// that uses all of its own meets Wasmtime's limit and never the end of the thread's stack, on any host.
const THREAD_STACK_BYTES: usize = WASM_STACK_BYTES + 2 * MIB as usize;

/// Fixed room in a run's memory beyond what it is charged for (`runtime/31` §6, D-53).
const MEMORY_OVERHEAD_BYTES: u64 = MIB;

/// The most pages a run's memory may have: under 4 GiB, whatever the limits are.
const MAX_PAGES: u64 = 65_535;

/// The `StoreLimits` figure (`runtime/31` §6, D-90, D-113): `max_memory`, the inputs' bytes, the data segment, the
/// scratch stack and the overhead, rounded up to pages, saturating.
pub(crate) const fn memory_limit(max_memory: u64, inputs: u32, data: u32) -> u64 {
    let bytes = max_memory
        .saturating_add(inputs as u64)
        .saturating_add(data as u64)
        .saturating_add(SCRATCH_BYTES as u64)
        .saturating_add(MEMORY_OVERHEAD_BYTES);
    let pages = bytes.div_ceil(PAGE_BYTES);
    (if pages > MAX_PAGES { MAX_PAGES } else { pages }) * PAGE_BYTES
}

/// The address space a run's memory reserves: the figure above at the system cap, not Wasmtime's 4 GiB (D-113,
/// D-120). A run whose inputs or literals take it past that moves its memory as it grows.
const MEMORY_RESERVATION_BYTES: u64 = memory_limit(MAX_MEMORY, 0, 0);

/// How often the ticker moves the epoch on (`runtime/31` §6).
const EPOCH_TICK: Duration = Duration::from_millis(10);

/// Compiled and linked modules a sandbox keeps in memory, the least recently used dropped first: twice the leaves one
/// run can call (`max_goal_calls`), so a run never compiles a module twice, while a process that loads many, a fuzz run
/// or a long test, stays near 100 MB at some 400 KB a module. Dropping one only means compiling it again.
pub(crate) const MAX_LINKED: usize = 2 * MAX_GOAL_CALLS as usize;

/// Why a module was not loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadError {
    /// `VL0801`: the module imports something outside the whitelist, named here as `module.name`. It was not
    /// compiled (R-SBX-09, R-SEC-02).
    Denied {
        /// The import.
        import: String,
    },
    /// `VL0607`: the module does not validate or compile, or the engine could not be made. A backend bug.
    Internal(String),
}

impl LoadError {
    /// The diagnostic code.
    pub fn code(&self) -> Code {
        match self {
            LoadError::Denied { .. } => Code::CapabilityDenied,
            LoadError::Internal(_) => Code::InternalError,
        }
    }

    /// The failure of the goal `goal` declared at `span`, worded as in `reference/90`.
    pub fn diagnostic(&self, goal: &str, span: Span) -> Diagnostic {
        match self {
            LoadError::Denied { import } => Diagnostic::new(
                Code::CapabilityDenied,
                span,
                format!("`{goal}` tried to use `{import}`, which goals aren't allowed to use."),
            ),
            LoadError::Internal(_) => Diagnostic::internal_error(),
        }
    }
}

fn internal(error: impl std::fmt::Display) -> LoadError {
    LoadError::Internal(error.to_string())
}

/// A host-side backstop that fired before the deterministic limit it stands behind (R-SBX-12): a backend bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Backstop {
    /// Wasmtime ran out of its fuel: reported as `VL0601`.
    Fuel,
    /// The resource limiter refused to grow the memory: reported as `VL0604`.
    Memory,
}

/// What one invocation on WASM gave: what the interpreter gives (INV-3), and how the backstops fared.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Run {
    /// The output, or how the run failed, with the elements being visited (R-BLT-07).
    pub result: Result<Value, Failure>,
    /// The Velme fuel and memory spent, read from the module's globals (R-SBX-03).
    pub spent: Spent,
    /// The backstop that fired, if one did (R-SBX-12).
    pub backstop: Option<Backstop>,
    /// The Wasmtime fuel the run was given and used.
    pub wasmtime_fuel: WasmtimeFuel,
    /// Whether `velme_run` was called. A run that never started failed in the host before the module ran any of the
    /// goal (its thread, its instance, its inputs): the caller may run the goal elsewhere. One that started is the
    /// goal's outcome, whatever it is (D-121).
    pub started: bool,
}

/// The Wasmtime fuel of one run, which the differential suite holds under a quarter of what was given (R-SBX-15).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WasmtimeFuel {
    /// [`backstop_fuel`] of the run's `max_fuel` and memory limit; 0 if the run never reached the module.
    pub given: u64,
    /// What the module used of it.
    pub used: u64,
}

impl WasmtimeFuel {
    /// Whether the run used at most a quarter of the backstop (R-SBX-15, D-115).
    pub fn within_a_quarter(&self) -> bool {
        self.used <= self.given / 4
    }
}

impl Run {
    /// A run the host could not make sense of: `VL0607`.
    fn internal() -> Run {
        Run {
            result: Err(velme_builtins::Error::Internal.into()),
            spent: Spent::default(),
            backstop: None,
            wasmtime_fuel: WasmtimeFuel::default(),
            started: false,
        }
    }
}

/// What a store holds for one invocation.
struct Host {
    guard: Guard,
    /// The run's watchdog, asked at each epoch deadline (D-115).
    interrupt: Interrupt,
}

/// The resource limiter of one invocation: one instance with one memory, which may grow to `memory` bytes, and no
/// table (`runtime/31` §6).
struct Guard {
    memory: usize,
}

impl ResourceLimiter for Guard {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= self.memory)
    }

    fn table_growing(&mut self, _current: usize, _desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(false)
    }

    fn instances(&self) -> usize {
        1
    }

    fn tables(&self) -> usize {
        0
    }

    fn memories(&self) -> usize {
        1
    }
}

/// The failure of an import, carried out of the module as the trap's own error (R-SBX-06, D-119).
#[derive(Debug)]
struct ImportFailure(velme_builtins::Error);

impl std::fmt::Display for ImportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a host import failed: {:?}", self.0)
    }
}

impl std::error::Error for ImportFailure {}

fn kept<T>(result: Result<T, velme_builtins::Error>) -> wasmtime::Result<T> {
    result.map_err(|error| wasmtime::Error::new(ImportFailure(error)))
}

/// The `Number` a module passed as `lo` and `hi`; one that is not in its canonical form is `VL0607` (R-SBX-04).
fn number(lo: i64, hi: i64) -> wasmtime::Result<Number> {
    kept(Number::from_bits(lo.cast_unsigned(), hi.cast_unsigned()))
}

fn bits(number: Number) -> (i64, i64) {
    let (lo, hi) = number.to_bits();
    (lo.cast_signed(), hi.cast_signed())
}

/// A built-in on numbers, as the interpreter calls it; charging it is the module's work (R-SBX-08).
fn builtin(function: Function, args: &[Number]) -> wasmtime::Result<Value> {
    let args: Vec<Value> = args.iter().copied().map(Value::Number).collect();
    Ok(kept(function.call(&args, u64::MAX, u64::MAX))?.value)
}

fn numeric(function: Function, args: &[Number]) -> wasmtime::Result<(i64, i64)> {
    match builtin(function, args)? {
        Value::Number(n) => Ok(bits(n)),
        _ => kept(Err(velme_builtins::Error::Internal)),
    }
}

type Arithmetic = fn(Number, Number) -> Result<Number, velme_builtins::Error>;

fn arithmetic(op: Arithmetic, (a, b): (i64, i64), (c, d): (i64, i64)) -> wasmtime::Result<(i64, i64)> {
    Ok(bits(kept(op(number(a, b)?, number(c, d)?))?))
}

fn unary(function: Function, a: i64, b: i64) -> wasmtime::Result<(i64, i64)> {
    numeric(function, &[number(a, b)?])
}

/// `velme.to_text`: the text of a number, written into the marshalling bytes of a scratch frame (R-SBX-11).
fn to_text(mut caller: Caller<'_, Host>, lo: i64, hi: i64, ptr: i32) -> wasmtime::Result<i32> {
    let refused = || kept(Err(velme_builtins::Error::Internal));
    let Value::Text(text) = builtin(Function::ToText, &[number(lo, hi)?])? else {
        return refused();
    };
    let ptr = ptr.cast_unsigned();
    let frame = ptr.checked_sub(FRAME_MARSHAL);
    let marshal = frame.is_some_and(|at| at.is_multiple_of(FRAME_BYTES) && at / FRAME_BYTES < FRAMES);
    let len = i32::try_from(text.len())
        .ok()
        .filter(|len| len.cast_unsigned() <= MARSHAL_BYTES);
    let (true, Some(len)) = (marshal, len) else {
        return refused();
    };
    let Some(memory) = caller.get_export(abi::MEMORY).and_then(Extern::into_memory) else {
        return refused();
    };
    match memory.write(&mut caller, ptr as usize, text.as_bytes()) {
        Ok(()) => Ok(len),
        Err(_) => refused(),
    }
}

/// The linker of the whitelist (`runtime/31` §5, R-SBX-09): these fourteen functions and nothing else, no WASI
/// (R-SBX-10). Each does a constant amount of work (D-112), with the `velme-builtins` code the interpreter calls.
fn linker(engine: &Engine) -> wasmtime::Result<Linker<Host>> {
    let mut linker = Linker::new(engine);
    for import in Import::ALL {
        let (module, name) = (abi::IMPORT_MODULE, import.name());
        match import {
            Import::NumAdd => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                arithmetic(Number::checked_add, (a, b), (c, d))
            }),
            Import::NumSub => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                arithmetic(Number::checked_sub, (a, b), (c, d))
            }),
            Import::NumMul => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                arithmetic(Number::checked_mul, (a, b), (c, d))
            }),
            Import::NumDiv => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                arithmetic(Number::checked_div, (a, b), (c, d))
            }),
            Import::NumNeg => linker.func_wrap(module, name, |a: i64, b: i64| Ok(bits(-number(a, b)?))),
            Import::NumCmp => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                // Exactly -1, 0 or 1 (D-119).
                Ok(match number(a, b)?.cmp(&number(c, d)?) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            }),
            Import::Abs => linker.func_wrap(module, name, |a: i64, b: i64| unary(Function::Abs, a, b)),
            Import::Floor => linker.func_wrap(module, name, |a: i64, b: i64| unary(Function::Floor, a, b)),
            Import::Ceil => linker.func_wrap(module, name, |a: i64, b: i64| unary(Function::Ceil, a, b)),
            Import::Round => linker.func_wrap(module, name, |a: i64, b: i64| unary(Function::Round, a, b)),
            Import::Clamp => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64, e: i64, f: i64| {
                numeric(Function::Clamp, &[number(a, b)?, number(c, d)?, number(e, f)?])
            }),
            Import::Random => linker.func_wrap(module, name, |a: i64, b: i64, c: i64, d: i64| {
                numeric(Function::Random, &[number(a, b)?, number(c, d)?])
            }),
            Import::ToText => linker.func_wrap(module, name, to_text),
            Import::RangeLen => linker.func_wrap(module, name, |a: i64, b: i64| {
                let length = kept(range_length(number(a, b)?))?;
                // At most `max_list_size`, which fits.
                kept(i32::try_from(length).map_err(|_| velme_builtins::Error::Internal))
            }),
        }?;
    }
    Ok(linker)
}

/// The engine every module runs on (`runtime/31` §6). Its configuration is part of a cached file's name (R-SBX-13).
fn engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    // Wasmtime's own fuel costs, one unit an instruction and one a byte a bulk-memory instruction moves: K and A
    // cover the instructions and M the bytes (D-120). Cranelift checks fuel and the epoch after each such instruction.
    config
        .consume_fuel(true)
        .epoch_interruption(true)
        .max_wasm_stack(WASM_STACK_BYTES)
        // Nothing here is async, but Wasmtime holds the stack above to this figure.
        .async_stack_size(WASM_STACK_BYTES)
        .memory_reservation(MEMORY_RESERVATION_BYTES)
        .memory_reservation_for_growth(MEMORY_OVERHEAD_BYTES)
        .memory_guard_size(PAGE_BYTES)
        .guard_before_linear_memory(false)
        .wasm_backtrace_max_frames(None)
        // R-SBX-07: exactly what `validate` accepts, floats, sign extension and saturating conversions off too, so
        // Wasmtime would refuse whatever `validate` does. Every feature starts off and only those are turned on;
        // Wasmtime needs none besides.
        .wasm_features(WasmFeatures::all(), false)
        .wasm_features(FEATURES, true);
    Engine::new(&config)
}

/// The sandbox: one engine, its linker of the whitelist, the thread that moves its epoch on, and where its compiled
/// modules are kept. Nothing public runs bytes the emitter did not produce (R-SBX-09).
///
/// A sandbox is made once per process: its disk cache, once refused, stays off (R-SBX-20, D-120), and each module
/// it loads is compiled once for the process.
pub struct Sandbox {
    engine: Engine,
    linker: Linker<Host>,
    cache: Option<Cache>,
    /// What each module loaded so far was compiled and linked to, by BLAKE3 of its bytes: code only, never a goal's
    /// signature or data, which two goals with the same bytes need not share.
    linked: Mutex<Linked>,
}

/// The modules kept in memory, by BLAKE3 of their bytes, each with when it was last used; at most [`MAX_LINKED`].
#[derive(Default)]
struct Linked {
    modules: HashMap<[u8; 32], (InstancePre<Host>, u64)>,
    clock: u64,
}

impl Linked {
    /// The module `key`, if it is kept, marked as just used.
    fn get(&mut self, key: &[u8; 32]) -> Option<InstancePre<Host>> {
        self.clock += 1;
        let (pre, used) = self.modules.get_mut(key)?;
        *used = self.clock;
        Some(pre.clone())
    }

    /// Keeps `pre` as the module `key`, or what another thread kept first, dropping the least recently used if full.
    fn keep(&mut self, key: [u8; 32], pre: InstancePre<Host>) -> InstancePre<Host> {
        if let Some(kept) = self.get(&key) {
            return kept;
        }
        if self.modules.len() >= MAX_LINKED {
            let oldest = self.modules.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k);
            if let Some(oldest) = oldest {
                self.modules.remove(&oldest);
            }
        }
        self.modules.insert(key, (pre.clone(), self.clock));
        pre
    }
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox")
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl Sandbox {
    /// A sandbox that keeps compiled modules in the directory `cache`, or only in memory for `None` (R-SBX-20).
    /// The directory is the user-level one, checked to lie outside the project (R-SBX-13, T-11, D-48).
    pub fn new(cache: Option<CacheDir>) -> Result<Sandbox, LoadError> {
        let engine = engine().map_err(internal)?;
        let linker = linker(&engine).map_err(internal)?;
        // One ticker for the engine: it holds the engine weakly, so it ends with the last module (D-115).
        let weak = engine.weak();
        let ticker = move || {
            loop {
                thread::sleep(EPOCH_TICK);
                match weak.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => break,
                }
            }
        };
        thread::Builder::new()
            .name("velme-epoch".to_owned())
            .spawn(ticker)
            .map_err(internal)?;
        let cache = cache.map(|dir| Cache::new(dir, &engine));
        Ok(Sandbox {
            engine,
            linker,
            cache,
            linked: Mutex::new(Linked::default()),
        })
    }

    /// How many compiled modules are kept in memory, for the test of [`MAX_LINKED`].
    #[cfg(test)]
    pub(crate) fn linked(&self) -> usize {
        self.linked.lock().unwrap_or_else(PoisonError::into_inner).modules.len()
    }

    /// Whether the module `bytes` is kept in memory, without marking it used, for the test of which one is dropped.
    #[cfg(test)]
    pub(crate) fn is_kept(&self, bytes: &[u8]) -> bool {
        let key = blake3::hash(bytes);
        self.linked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .modules
            .contains_key(key.as_bytes())
    }

    /// The disk cache, for the tests of what it reads.
    #[cfg(test)]
    pub(crate) fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Why the disk cache is off, if a directory was given and it is (R-SBX-20, D-120): a note for `--verbose`.
    /// Every module is then compiled on each run; nothing else changes.
    pub fn cache_off(&self) -> Option<String> {
        let (dir, why) = self.cache.as_ref()?.off()?;
        Some(format!(
            "the compiled-module cache in `{}` is not used, since {why}; modules are compiled on every run",
            dir.display()
        ))
    }

    /// The emitted `module`, compiled and linked, ready to run any number of times. Emission always runs first;
    /// only what Cranelift made of the module is cached (R-SBX-13), and in memory only its code: the program takes
    /// the signature, data and literals of `module` itself.
    pub fn load(&self, module: &Module) -> Result<Program, LoadError> {
        self.load_bytes(module.bytes(), module)
    }

    /// The module `bytes`, with the signature and data segment of `like`. It takes any bytes, so it stays private to
    /// the crate (R-SBX-09): they are validated (R-SBX-07), then their imports are held against the whitelist, and
    /// only then compiled.
    pub(crate) fn load_bytes(&self, bytes: &[u8], like: &Module) -> Result<Program, LoadError> {
        let key = *blake3::hash(bytes).as_bytes();
        let known = self.linked.lock().unwrap_or_else(PoisonError::into_inner).get(&key);
        let pre = match known {
            Some(pre) => pre,
            None => {
                validate(bytes).map_err(LoadError::Internal)?;
                whitelisted(bytes)?;
                let compiled = self.compiled(bytes)?;
                let pre = self.linker.instantiate_pre(&compiled).map_err(internal)?;
                let mut linked = self.linked.lock().unwrap_or_else(PoisonError::into_inner);
                linked.keep(key, pre)
            }
        };
        Ok(Program {
            pre,
            signature: like.signature.clone(),
            data_bytes: like.data_bytes(),
            literals: like.literals,
        })
    }

    /// What Cranelift makes of `bytes`: from the cache if it is there, else compiled and kept.
    fn compiled(&self, bytes: &[u8]) -> Result<wasmtime::Module, LoadError> {
        let Some(cache) = &self.cache else {
            return wasmtime::Module::from_binary(&self.engine, bytes).map_err(internal);
        };
        let path = cache.path(bytes);
        if let Some(module) = self.cached(cache, &path) {
            return Ok(module);
        }
        let module = wasmtime::Module::from_binary(&self.engine, bytes).map_err(internal)?;
        if let Ok(serialized) = module.serialize() {
            cache.write(&path, &serialized);
        }
        Ok(module)
    }

    /// The compiled module kept at `path`, if `path` is a file of the cache directory that this engine wrote. One
    /// that does not load is deleted, and the caller compiles (R-SBX-14). Off Unix there is no disk cache (D-120).
    #[cfg(unix)]
    fn cached(&self, cache: &Cache, path: &Path) -> Option<wasmtime::Module> {
        // A refusal has turned the disk cache off (R-SBX-20); the caller compiles.
        let bytes = cache.read(path).ok()??;
        // SAFETY: a compiled module is native code that Wasmtime runs as it is, so the bytes must be ones Wasmtime
        // made. What `Cache::read` checked, and nothing more (Unix only; there is no disk cache elsewhere): the
        // directory given to the sandbox, a `CacheDir` and so absolute, with no `.` or `..` component, and outside the
        // project resolved both when it was given and again once made, just before this open (R-SBX-14, T-11), opened
        // without following a symbolic link in its last component, is, by `fstat` of that handle, a directory owned by
        // this process's effective user with no permission bit for group or others; the file, a name of the form
        // `<BLAKE3 of the module>-<compatibility hash>.cwasm` directly in it, opened through that handle without
        // following a symbolic link, is, by `fstat` of its own handle, a regular file of the same owner that neither
        // group nor others can write (R-SBX-20, D-120). The bytes are read from that handle into memory, so no swap
        // after the checks reaches them. Only this user, or root, could have put them there; Velme writes a file there
        // only whole, `0600`, by rename. Extended ACLs (macOS) are not checked: only the owner or root can set one on a
        // `0700` directory, so they are out of scope (T-11). Wasmtime checks again that it was made by this version and
        // configuration of the engine, which is no defence against crafted bytes.
        #[allow(unsafe_code)]
        let module = unsafe { wasmtime::Module::deserialize(&self.engine, &bytes) };
        if module.is_err() {
            cache.discard(path);
        }
        module.ok()
    }

    #[cfg(not(unix))]
    fn cached(&self, _cache: &Cache, _path: &Path) -> Option<wasmtime::Module> {
        None
    }
}

/// `VL0801` for the first import of the module `bytes` that the whitelist does not have (R-SBX-09, INV-4).
fn whitelisted(bytes: &[u8]) -> Result<(), LoadError> {
    for payload in Parser::new(0).parse_all(bytes) {
        let Payload::ImportSection(section) = payload.map_err(internal)? else {
            continue;
        };
        for import in section.into_imports() {
            let import = import.map_err(internal)?;
            let allowed = import.module == abi::IMPORT_MODULE && Import::ALL.iter().any(|i| i.name() == import.name);
            if !allowed {
                return Err(LoadError::Denied {
                    import: format!("{}.{}", import.module, import.name),
                });
            }
        }
    }
    Ok(())
}

/// A loaded module: compiled once, run in a fresh store and instance each time, so no state passes between two
/// invocations (`runtime/31` §6).
pub struct Program {
    pre: InstancePre<Host>,
    signature: Signature,
    data_bytes: u32,
    literals: Literals,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program").finish_non_exhaustive()
    }
}

/// What the host uses of an instance (R-SBX-03).
struct Exports {
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    run: TypedFunc<i32, i32>,
    fuel_left: Global,
    memory_left: Global,
    reason: Global,
}

impl Exports {
    fn of(instance: &Instance, store: &mut Store<Host>) -> Option<Exports> {
        Some(Exports {
            memory: instance.get_memory(&mut *store, abi::MEMORY)?,
            alloc: instance.get_typed_func(&mut *store, abi::ALLOC).ok()?,
            run: instance.get_typed_func(&mut *store, abi::RUN).ok()?,
            fuel_left: instance.get_global(&mut *store, abi::FUEL_LEFT)?,
            memory_left: instance.get_global(&mut *store, abi::MEMORY_LEFT)?,
            reason: instance.get_global(&mut *store, abi::REASON)?,
        })
    }
}

impl Program {
    /// Runs the goal on `inputs`, one per input in declared order, within `limits`, stopped from outside by
    /// `interrupt`, the run's watchdog: the value, fuel and memory the interpreter gives, or the same failure
    /// (INV-3). `VL0603` is the one outcome that is not reproducible (D-10).
    ///
    /// `limits` must be within [`Limits::SYSTEM`], as the runtime's always are: the engine reserves the address space
    /// of a run at the system cap (D-120), and a run given more costs a move of its memory each time it grows.
    ///
    /// # Panics
    ///
    /// Only if the host panics on the run's thread: that panic goes on in the caller's thread.
    pub fn run(&self, inputs: &[Value], limits: Limits, interrupt: &Interrupt) -> Run {
        // A thread of its own, so the module has the whole of its stack whatever thread asks (D-113, D-120).
        thread::scope(|scope| {
            let spawned = thread::Builder::new()
                .name("velme-wasm".to_owned())
                .stack_size(THREAD_STACK_BYTES)
                .spawn_scoped(scope, || self.invoke(inputs, limits, interrupt));
            match spawned.map(thread::ScopedJoinHandle::join) {
                Ok(Ok(run)) => run,
                // A bug of the host, not of the module: it is not hidden as `VL0607`.
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(_) => Run::internal(),
            }
        })
    }

    fn invoke(&self, inputs: &[Value], limits: Limits, interrupt: &Interrupt) -> Run {
        let mut run = Run::internal();
        let Ok(image) = Image::new(&self.signature, inputs) else {
            return run;
        };
        let memory = memory_limit(limits.memory, image.len(), self.data_bytes);
        run.wasmtime_fuel.given = backstop_fuel(limits.fuel, memory);
        let host = Host {
            guard: Guard {
                memory: usize::try_from(memory).unwrap_or(usize::MAX),
            },
            interrupt: interrupt.clone(),
        };
        let mut store = Store::new(self.pre.module().engine(), host);
        store.limiter(|host| &mut host.guard);
        store.epoch_deadline_callback(|context| {
            Ok(if context.data().interrupt.stopped() {
                UpdateDeadline::Interrupt
            } else {
                UpdateDeadline::Continue(1)
            })
        });
        store.set_epoch_deadline(1);
        if store.set_fuel(run.wasmtime_fuel.given).is_err() {
            return run;
        }
        let Ok(instance) = self.pre.instantiate(&mut store) else {
            return run;
        };
        let Some(exports) = Exports::of(&instance, &mut store) else {
            return run;
        };
        // What the result may take, charged or not (R-SBX-11, D-120): the inputs and the literals are nobody's
        // charge (D-53, D-83). An item of a list of `Nothing` has no size; each one a run makes costs a unit of fuel.
        let bytes = limits
            .memory
            .saturating_add(image.logical())
            .saturating_add(self.literals.bytes)
            .saturating_add(RESULT_SLACK_BYTES);
        let sizeless = image.sizeless().saturating_add(self.literals.sizeless);
        let Ok(base) = prepare(&exports, &mut store, image, limits) else {
            return run;
        };
        // The first deadline is the call itself: a run that starts past its time is stopped before it runs, and from
        // then on the watchdog is asked at every tick (D-115).
        store.set_epoch_deadline(0);
        run.started = true;
        let called = exports.run.call(&mut store, base);
        run.wasmtime_fuel.used = run.wasmtime_fuel.given.saturating_sub(store.get_fuel().unwrap_or(0));
        let left = |global: &Global, store: &mut Store<Host>| global.get(store).i64().map(i64::cast_unsigned);
        let fuel_left = left(&exports.fuel_left, &mut store);
        let memory_left = left(&exports.memory_left, &mut store);
        let reason = exports.reason.get(&mut store).i32();
        let (Some(fuel_left), Some(memory_left), Some(reason)) = (fuel_left, memory_left, reason) else {
            return run;
        };
        // A module never has more left than it was given (R-SBX-11).
        if fuel_left > limits.fuel || memory_left > limits.memory {
            return run;
        }
        run.spent = Spent {
            fuel: limits.fuel - fuel_left,
            memory: limits.memory - memory_left,
        };
        let memory = exports.memory.data(&store);
        match called {
            // A module that raised something does not return (R-SBX-11).
            Ok(at) if reason == Reason::None.code() => {
                let items = sizeless.saturating_add(run.spent.fuel);
                let mut reader = Reader::new(memory, &self.signature.types, bytes, items);
                if let Ok(value) = reader.output(&self.signature.output, at.cast_unsigned()) {
                    run.result = Ok(value);
                }
            }
            Ok(_) => {}
            Err(error) => {
                let left = (fuel_left, memory_left);
                if let Some((error, backstop)) = classify(&error, reason, left, limits)
                    && let Some(elements) = visiting(memory)
                {
                    run.result = Err(Failure { error, elements });
                    run.backstop = backstop;
                }
            }
        }
        run
    }
}

/// Room in the result's budget for an output that is a scalar, which nobody is charged for (D-89).
const RESULT_SLACK_BYTES: u64 = 64;

/// Sets the limits and writes the inputs: the address to call `velme_run` with (R-SBX-03).
fn prepare(exports: &Exports, store: &mut Store<Host>, image: Image, limits: Limits) -> wasmtime::Result<i32> {
    exports
        .fuel_left
        .set(&mut *store, Val::I64(limits.fuel.cast_signed()))?;
    exports
        .memory_left
        .set(&mut *store, Val::I64(limits.memory.cast_signed()))?;
    let corrupt = || wasmtime::Error::msg("the inputs do not fit the module's memory");
    let base = exports.alloc.call(&mut *store, image.len().cast_signed())?;
    let bytes = image.placed(base.cast_unsigned()).map_err(|_| corrupt())?;
    exports
        .memory
        .write(&mut *store, base.cast_unsigned() as usize, &bytes)?;
    Ok(base)
}

/// Why the module stopped, in the order of R-SBX-11, and the backstop that fired if one did; `None` for `VL0607`.
fn classify(
    error: &wasmtime::Error,
    reason: i32,
    (fuel_left, memory_left): (u64, u64),
    limits: Limits,
) -> Option<(Error, Option<Backstop>)> {
    let out_of_fuel = Error::OutOfFuel { max_fuel: limits.fuel };
    let out_of_memory = Error::OutOfMemory {
        max_memory: limits.memory,
    };
    if let Some(ImportFailure(kept)) = error.downcast_ref() {
        return Some((Error::Builtin(kept.clone()), None));
    }
    match error.downcast_ref::<Trap>()? {
        Trap::Interrupt => Some((Error::Interrupted, None)),
        Trap::OutOfFuel => Some((out_of_fuel, Some(Backstop::Fuel))),
        Trap::UnreachableCodeReached => match Reason::from_code(reason)? {
            // A limit the module hit leaves its global at 0 (R-SBX-05).
            Reason::OutOfFuel if fuel_left == 0 => Some((out_of_fuel, None)),
            Reason::OutOfMemory if memory_left == 0 => Some((out_of_memory, None)),
            Reason::GrowRefused => Some((out_of_memory, Some(Backstop::Memory))),
            Reason::None | Reason::OutOfFuel | Reason::OutOfMemory | Reason::Internal => None,
        },
        _ => None,
    }
}

/// The index of the element each collection node was visiting when the module stopped, innermost first
/// (R-BLT-07, R-SBX-19); `None` if a frame holds what no run leaves there.
fn visiting(memory: &[u8]) -> Option<Vec<usize>> {
    let mut elements = Vec::new();
    for level in (0..FRAMES).rev() {
        let at = (abi::frame(level) + FRAME_VISITING) as usize;
        let cell = memory.get(at..at + 4)?.first_chunk()?;
        match u64::from(u32::from_le_bytes(*cell)) {
            0 => {}
            next if next <= MAX_LIST_SIZE => elements.push(usize::try_from(next - 1).ok()?),
            _ => return None,
        }
    }
    Some(elements)
}
