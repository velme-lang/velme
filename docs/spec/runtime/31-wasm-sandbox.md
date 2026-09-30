# 31 — WASM Backend & Wasmtime Sandbox

**Status:** v0.1 (roadmap M7, the last MVP phase) · **Area:** SBX
**Read when:** working on IR → WASM code generation, the value layout, Wasmtime configuration, host imports, or interpreter/WASM differential tests.
**Depends on:** [SPEC](../SPEC.md), [compiler/21](../compiler/21-ir.md), [30-execution-vibevm](30-execution-vibevm.md), [language/14](../language/14-builtins.md), [tooling/41](../tooling/41-security-privacy.md)

## 1. Purpose & boundaries

The WASM backend compiles a verified IR goal body to a core WebAssembly module and runs it in Wasmtime with no
ambient capabilities. It is an **optimization backend** (P-4): the interpreter ([30](30-execution-vibevm.md) §3)
defines semantics and WASM must produce the same value, the same full diagnostic on failure, and the same fuel and
memory figures (INV-3, D-118). Composite-goal scheduling stays in the host runtime; only leaf goal bodies become
modules.

## 2. Strategy

| Stage | v0.1 | Later |
|---|---|---|
| Unit of compilation | one **leaf** goal body → one core module; composite tails stay on the interpreter | composite tails, whole-program modules |
| Interface | fixed Velme value layout in linear memory (§3) | Component Model + WIT records/lists (Future) |
| Composition | host-managed DAG (30 §4) | component composition (Future) |
| Emission | `wasm-encoder` | — |
| Validation | `wasmparser` with a fixed feature set (§4) | — |
| Runtime | Wasmtime, embedded, no WASI | WASI components with explicit capabilities (Future, `effects`) |

**R-SBX-01** WASM is never produced by an LLM (INV-1); only `velme-wasm` emits it, only from validated IR.
**R-SBX-02** Backend selection: `--backend interp|wasm|auto`, a flag only, with no `velme.toml` key (D-117). The
default is `interp` until the M7 gate passes, then `auto`. `auto` runs a leaf goal on WASM when the emitter accepts it
and on the interpreter otherwise. `wasm` does the same, except that a leaf the emitter declines is `VL0607`, never a
silent fallback. On a composite goal both run the leaves on WASM and the tail on the interpreter.
**R-SBX-17** Only leaf goal bodies in `run`, `test` and `trace` use WASM. Checks, examples, call arguments, composite
tails, build verification and `--locked` re-verification stay on the interpreter (D-80, D-117).
**R-SBX-18** Output is byte-identical for all three values: stdout, the diagnostics on stderr, the exit code, `--json`,
and `trace --json` with its fuel and memory figures, for a success and for every deterministic failure. Timings and
`VL0603` are excluded. Neither `velme-cli/1` nor the trace has a `backend` field (D-117).

## 3. Value layout (ABI)

Chosen: a **type-directed in-memory layout** that both sides share. The host writes inputs directly into the module's
linear memory in this layout and reads the result back; the module never parses JSON or a tagged format.

| Type | Layout (little-endian, 8-byte aligned slots) |
|---|---|
| `Number` | 16-byte, 8-aligned slot: two `i64`, `lo` then `hi`. `lo` = magnitude bits 0..63; `hi` = magnitude bits 64..95 in its low 32 bits, the scale in bits 32..39, the sign in bit 63. One canonical form per value (D-113) |
| `Boolean` | `i32` 0/1 |
| `Nothing` | zero-size |
| `T?` | `i32` tag in its own 8-byte slot (0 = nothing, 1 = present) + `T` slot, so `Number?` is exactly 24 bytes |
| `Text` | `i32 ptr, i32 len` → UTF-8 bytes |
| `List<T>` | `i32 ptr, i32 len` → `len` contiguous `T` slots |
| record | fields in declared order, each in its slot |

Why: the types are statically known on both sides, so tags and a decoder inside WASM are unnecessary; emitting
typed loads/stores is simpler than emitting a parser, and the encoding is canonical by construction (deterministic).

**R-SBX-03** Module exports exactly: `memory`, `velme_alloc(size: i32) -> i32`, `velme_run(input_ptr: i32) -> i32`
(pointer to the result slot), and three mutable globals, `velme_fuel_left`, `velme_memory_left` and `velme_reason`
(D-113). The host sets the first two from the run's limits before it calls `velme_run`, and reads all three afterwards,
also after a trap: fuel used, memory used and the trap reason come from them. Limits are never baked into a module, so
a module is a pure function of its IR. Allocation is a bump allocator; nothing is freed — each invocation gets a fresh
instance.
**R-SBX-19** Linear memory holds only (a) the values 30 §7.1 charges, each in no more than its charged size,
bump-allocated; (b) the invocation's inputs, written by the host; (c) literal data segments; (d) a fixed scratch stack
of `MAX_COLLECTION_NESTING + 1` frames, each sized for `MAX_LIST_SIZE` and never bump-allocated. A collection node
uses the frame at its nesting depth, so a `sort_by` key that holds another `filter` or `sort_by` has a frame of its
own. The frames hold sort order and keys, filter selection, the result slot and host-call marshalling. S, the size of
the whole stack, is one named constant. Nothing else takes memory that grows with fuel (D-90, D-113).
**R-SBX-04** The host validates every pointer/length it reads back against memory bounds; an out-of-range result, or a
`Number` that is not in its canonical form (D-113), is `VL0607` (a backend bug, never user-visible data corruption).

## 4. Code generation & validation

**R-SBX-05** The emitter instruments Velme fuel and memory explicitly: each IR node takes its cost from the shared cost
table (30 R-RUN-04) off `velme_fuel_left`; each value creation takes the 30 §7.1 size off `velme_memory_left`, so
memory used is the cumulative bytes allocated (30 §7.1, D-53). Crossing a limit sets `velme_reason` and traps; the host
maps the reason to `VL0601`/`VL0604` (D-113).
The cost table is one constant set in `velme-builtins`, depended on by both `velme-interp` and `velme-wasm` (D-54).
**R-SBX-06** `Number` arithmetic and comparison are host imports (`velme.num_add`, `num_sub`, `num_mul`, `num_div`,
`num_neg`, `num_cmp`) implemented by the same `velme-builtins` code as the interpreter, so results match bit for bit
(D-36). Division by zero or overflow returns the `VL0602` reason and the module traps. Numbers cross these imports, and
the other imports of §5, as values — two `i64`, `lo` then `hi` (§3), never through linear memory (D-90, D-112).
**R-SBX-07** The emitter uses only: MVP instructions except any `f32`/`f64` operation (`Number` is decimal, D-36), multi-value, bulk
memory, mutable globals. No threads, SIMD,
relaxed SIMD, reference types, exceptions or tail calls in v0.1. `wasmparser` validates every module with exactly this
feature set before it is cached or instantiated.
**R-SBX-08** Host imports, under module `velme`, do scalar work only: each does a constant amount of host work per
call, so Velme fuel stays a true bound on the run (D-112). They are the imports of §5, implemented by `velme-builtins` —
the same Rust code the interpreter calls — so their semantics cannot diverge. Everything that walks a `List` or a
`Text` is emitted WASM: the list and text builtins, equality on composite values, and the collection nodes (`map`,
`filter`, `find`, `reduce`, `all`, `any`, `sort_by`); `sort_by` is an emitted stable merge sort. Emitted code charges
fuel and memory from the shared cost constants (R-SBX-05), and the differential suite (§8) holds it equal to the
interpreter.

## 5. Host imports (whitelist)

The whitelist is this table and nothing else (D-112). `n` is a `Number` passed as two `i64`, `lo` then `hi` (§3). Every
v0.1 import is pure and deterministic, and is charged as the IR node or builtin that uses it.

| Import | Signature | v0.1 | Notes |
|---|---|---|---|
| `velme.num_add`, `num_sub`, `num_mul`, `num_div` | `(n, n) -> n` | yes | R-SBX-06; overflow or division by zero is `VL0602` |
| `velme.num_neg` | `(n) -> n` | yes | R-SBX-06 |
| `velme.num_cmp` | `(n, n) -> i32` | yes | the order of the two numbers |
| `velme.abs`, `floor`, `ceil`, `round` | `(n) -> n` | yes | language/14 |
| `velme.clamp` | `(n, n, n) -> n` | yes | `x`, `low`, `high`; `low > high` is `VL0602` |
| `velme.random` | `(n, n) -> n` | yes | `seed`, `index` (D-22) |
| `velme.to_text` | `(n, ptr: i32) -> i32` | yes | writes the text into the scratch frame at `ptr`, returns its byte length |
| `velme.range_len` | `(n) -> i32` | yes | checks the argument of `range` (`VL0602`, `VL0606`) and returns the length; the list is built by emitted code |
| `velme.text_chars` | `(ptr: i32, len: i32) -> n` | yes | the count of Unicode scalar values in a `Text`, for `length` |
| `velme.log` | — | Future | would be an effect |
| `velme.now` | — | Future | requires `effects: clock` |
| `velme.call` | — | Future | composite goals stay host-scheduled |

**R-SBX-09** Because `random` is a pure builtin (D-22), no effectful host function exists in v0.1. The `Linker`
defines only the imports of this table. After `wasmparser` validation (R-SBX-07) and before Wasmtime compiles a module,
the host scans the module's imports against the whitelist: an import outside it is `VL0801` naming the import, and the
module is neither compiled nor run (INV-4, D-116). The entry point that takes raw module bytes is private to
`velme-wasm` and tested inside it; no public API runs bytes the emitter did not produce.
**R-SBX-10** No WASI is linked: no filesystem, network, environment, clock or process (INV-4).

## 6. Wasmtime configuration

| Setting | Value | Why |
|---|---|---|
| `consume_fuel` | on, set to the Velme fuel budget × a named constant, which comes from the most instructions the emitter lets one Velme fuel unit cover | backstop only; Velme fuel (R-SBX-05) is the deterministic limit, and a legal program never reaches the backstop (D-115) |
| `epoch_interruption` | on; ticker thread increments every 10 ms; the deadline is one tick, and at each deadline a callback asks the run's shared watchdog (30 §7), which reads the injected clock: it either allows one more tick or stops the module | wall-clock safety net → `VL0603` (D-10, D-51, D-115); tests drive it with the fake clock |
| `wasm_threads`, `wasm_simd`, `wasm_relaxed_simd` | off | determinism (R-SBX-07) |
| `StoreLimits` | memory ≤ `max_memory` + the ABI bytes of the invocation's inputs + the data-segment bytes + S (R-SBX-19) + a fixed overhead of 1 MiB, rounded up to 64 KiB pages; 1 memory, 1 table, 1 instance | host-side backstop behind the deterministic memory counter → `VL0604` (D-53, D-90, D-113) |
| Memory reservation | equal to the `StoreLimits` memory figure, not Wasmtime's 4 GiB default | no address space beyond what a run may use (D-113) |
| WASM stack | 4 MiB | fixed, so deep emitted code fails the same way on every host (D-113) |
| `wasmtime` crate | default features off; only `runtime`, `cranelift` and `std`; always built in, with no cargo feature to turn it off | small dependency surface; one build of `velme` (D-116) |
| Instantiation | `InstancePre` per module, fresh `Store` + instance per invocation | no state shared between calls |

**R-SBX-11** A Wasmtime trap is mapped by reason: Velme fuel/memory/arithmetic reason codes → their `VL06xx`;
Wasmtime fuel exhaustion → `VL0601`; epoch deadline → `VL0603`; `ResourceLimiter` denial → `VL0604`; anything else →
`VL0607 InternalError` (and is a bug).
**R-SBX-12** Only the deterministic limits (Velme fuel, Velme memory) can produce reproducible outcomes; if a backstop
fires first, the run is reported as that backstop's code. `--verbose` adds a note on stderr saying that a backstop
fired, which is a backend bug; nothing else changes, in `--json` or in the trace (D-115).

## 7. Compiled-module cache

**R-SBX-13** Compiled modules are a **derived** cache (D-12) kept in a **user-level** directory, never inside the
project: `$XDG_CACHE_HOME/velme/wasm/<module-hash>-<compat-hash>.cwasm` (or the platform equivalent), where
`module-hash` = BLAKE3 of the emitted module's bytes and `compat-hash` is Wasmtime's compatibility hash for the engine,
so a change to the emitter, to Wasmtime or to the engine configuration gives a new name (D-116). Emission always runs;
only Wasmtime's compiled output is cached. They are never locked, never committed and may be deleted at any time (D-48).
**R-SBX-20** The cache directory is an option passed to the backend: `velme-cli` passes the user-level directory, and a
test passes a temporary directory or none (no disk cache). The directory is created with mode `0700`; a symbolic link,
as the directory or as a file in it, is refused; a file is written under a temporary name and renamed into place
atomically (D-116).
**R-SBX-14** `Module::deserialize` is used only on files under the configured cache directory, written by this
process's engine configuration: the loader refuses any path outside that directory, and a name mismatch or read error
falls back to compiling the emitted module. Files anywhere under
the project — including `.velme/` — are never passed to `Module::deserialize` (D-48, T-11): a `.cwasm` planted in a
cloned project is never read and the goal is compiled fresh from its validated IR. CI must not restore this cache
across a trust boundary (e.g. from a fork's PR onto a shared runner).

## 8. Differential testing (INV-3)

**R-SBX-15** Every golden IR, every example and every IR from the typed valid-IR generator of `velme-test-support`
(shared by proptest and the `differential` fuzz target) runs on both backends. The gate compares exactly: the result
value (canonical JSON), the full diagnostic of a failure (code, message and notes), Velme fuel used and Velme memory
used (cumulative bytes allocated, D-53). It also asserts that no run uses over 25 % of its Wasmtime fuel backstop
(§6, D-115). Any difference fails the gate (`delivery/51`, D-118).
**R-SBX-16** A new IR node kind or builtin is not released for the WASM backend until it passes the differential suite;
until then the emitter declines the goal, `auto` uses the interpreter and an explicit `wasm` is `VL0607` (R-SBX-02).

## 9. Future: Component Model

Goals as WASM components with WIT interfaces (records/lists natural across languages), component composition for
composite goals, and WASI capabilities tied to declared `effects`. Tracked by RFC; must not weaken INV-3/INV-4.

## 10. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SBX-01 | Every golden leaf goal gives the identical value or, when it fails, the identical full diagnostic (code, message and notes such as "tried to divide 1 by 0" or "at item 3 of a list"), and identical fuel and memory used (cumulative bytes allocated) on `interp` and `wasm` (D-118). |
| AC-SBX-02 | A module importing a non-whitelisted function (e.g. `wasi_snapshot_preview1.fd_write`) fails with `VL0801` naming the import, and is never compiled or run (D-116). |
| AC-SBX-03 | An expensive leaf is stopped with `VL0601` on WASM with the same fuel figure as the interpreter — AC-RDM-07. |
| AC-SBX-04 | A leaf that allocates past `max_memory` fails with `VL0604` on both backends at the same point. |
| AC-SBX-05 | Division by zero traps with `VL0602` on WASM; a module containing any `f32`/`f64` instruction is rejected by validation. |
| AC-SBX-06 | Every emitted module in the golden set validates with `wasmparser` under exactly the R-SBX-07 feature set. |
| AC-SBX-07 | Deleting the WASM cache directory changes no result; the next run recompiles, seen as the cache file appearing again. Tested with a temporary cache directory (R-SBX-20). |
| AC-SBX-08 | With the injected clock past `max_wall_clock`, a run on WASM ends with `VL0603`, flagged non-reproducible (D-115). |
| AC-SBX-09 | A garbage `.cwasm` file planted under the project's `.velme/` is never read: the goal compiles from IR and runs normally, and the loader refuses any path outside the configured cache directory (D-48, D-116). |
