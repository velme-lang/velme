# 31 — WASM Backend & Wasmtime Sandbox

**Status:** v0.1 (roadmap M7, the last MVP phase) · **Area:** SBX
**Read when:** working on IR → WASM code generation, the value layout, Wasmtime configuration, host imports, or interpreter/WASM differential tests.
**Depends on:** [SPEC](../SPEC.md), [compiler/21](../compiler/21-ir.md), [30-execution-vibevm](30-execution-vibevm.md), [language/14](../language/14-builtins.md), [tooling/41](../tooling/41-security-privacy.md)

## 1. Purpose & boundaries

The WASM backend compiles a verified IR goal body to a core WebAssembly module and runs it in Wasmtime with no
ambient capabilities. It is an **optimization backend** (P-4): the interpreter ([30](30-execution-vibevm.md) §3)
defines semantics and WASM must produce the same value, outcome code, fuel and memory figures (INV-3). Composite-goal
scheduling stays in the host runtime; only goal bodies become modules.

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
**R-SBX-02** Backend selection: `--backend interp|wasm|auto`. Default is `interp` until the M7 gate passes, then
`auto` (WASM for leaf goals with a compiled module, interpreter otherwise). Results are identical for all three.

## 3. Value layout (ABI)

Chosen: a **type-directed in-memory layout** that both sides share. The host writes inputs directly into the module's
linear memory in this layout and reads the result back; the module never parses JSON or a tagged format.

| Type | Layout (little-endian, 8-byte aligned slots) |
|---|---|
| `Number` | 16-byte slot holding the canonical 128-bit decimal encoding of `velme-builtins` (coefficient + scale + sign) |
| `Boolean` | `i32` 0/1 |
| `Nothing` | zero-size |
| `T?` | `i32` tag (0 = nothing, 1 = present) + `T` slot |
| `Text` | `i32 ptr, i32 len` → UTF-8 bytes |
| `List<T>` | `i32 ptr, i32 len` → `len` contiguous `T` slots |
| record | fields in declared order, each in its slot |

Why: the types are statically known on both sides, so tags and a decoder inside WASM are unnecessary; emitting
typed loads/stores is simpler than emitting a parser, and the encoding is canonical by construction (deterministic).

**R-SBX-03** Module exports exactly: `memory`, `velme_alloc(size: i32) -> i32`, `velme_run(input_ptr: i32) -> i32`
(pointer to the result slot). Allocation is a bump allocator; nothing is freed — each invocation gets a fresh instance.
**R-SBX-04** The host validates every pointer/length it reads back against memory bounds; an out-of-range result is
`VL0607` (a backend bug, never user-visible data corruption).

## 4. Code generation & validation

**R-SBX-05** The emitter instruments Velme fuel and memory explicitly: each IR node decrements a global fuel counter by
its cost from the shared cost table (30 R-RUN-04); each value creation adds the 30 §7.1 size to a cumulative
bytes-allocated counter (30 §7.1, D-53). Crossing a limit traps with a reason code the host maps to `VL0601`/`VL0604`.
The cost table is one constant set in `velme-builtins`, depended on by both `velme-interp` and `velme-wasm` (D-54).
**R-SBX-06** `Number` arithmetic and comparison are host imports (`velme.num_add`, `num_sub`, `num_mul`, `num_div`,
`num_neg`, `num_cmp`) implemented by the same `velme-builtins` code as the interpreter, so results match bit for bit
(D-36). Division by zero or overflow returns the `VL0602` reason and the module traps.
**R-SBX-07** The emitter uses only: MVP instructions except any `f32`/`f64` operation (`Number` is decimal, D-36), multi-value, bulk
memory, mutable globals. No threads, SIMD,
relaxed SIMD, reference types, exceptions or tail calls in v0.1. `wasmparser` validates every module with exactly this
feature set before it is cached or instantiated.
**R-SBX-08** Builtins are **host imports** under module `velme`, implemented by `velme-builtins` — the same Rust code the
interpreter calls — so their semantics cannot diverge. Collection nodes (`map`, `filter`, `find`, `reduce`, `sort`)
are emitted in WASM; `sort` is an emitted stable merge sort.

## 5. Host imports (whitelist)

| Import | v0.1 | Notes |
|---|---|---|
| `velme.<builtin>` for each catalog entry of `builtins_version` (language/14), incl. `random(seed, index)` | yes | pure; deterministic; charged fuel per catalog |
| `velme.num_*` decimal arithmetic (R-SBX-06) | yes | pure; charged as the IR node that uses it |
| `velme.log` | Future | would be an effect |
| `velme.now` | Future | requires `effects: clock` |
| `velme.call` | Future | composite goals stay host-scheduled |

**R-SBX-09** Because `random` is a pure builtin (D-22), no effectful host function exists in v0.1. The `Linker`
defines only the `velme.*` catalog imports; a module importing anything else fails instantiation with `VL0801` (INV-4).
**R-SBX-10** No WASI is linked: no filesystem, network, environment, clock or process (INV-4).

## 6. Wasmtime configuration

| Setting | Value | Why |
|---|---|---|
| `consume_fuel` | on, set to 20 × the Velme fuel budget | backstop only; Velme fuel (R-SBX-05) is the deterministic limit |
| `epoch_interruption` | on; ticker thread increments every 10 ms; deadline = `max_wall_clock` | wall-clock safety net → `VL0603` (D-10) |
| `wasm_threads`, `wasm_simd`, `wasm_relaxed_simd` | off | determinism (R-SBX-07) |
| `StoreLimits` | memory ≤ `max_memory` + 1 MiB fixed overhead, 1 memory, 1 table, 1 instance | host-side backstop behind the deterministic memory counter → `VL0604` (D-53) |
| Instantiation | `InstancePre` per module, fresh `Store` + instance per invocation | no state shared between calls |

**R-SBX-11** A Wasmtime trap is mapped by reason: Velme fuel/memory/arithmetic reason codes → their `VL06xx`;
Wasmtime fuel exhaustion → `VL0601`; epoch deadline → `VL0603`; `ResourceLimiter` denial → `VL0604`; anything else →
`VL0607 InternalError` (and is a bug).
**R-SBX-12** Only the deterministic limits (Velme fuel, Velme memory) can produce reproducible outcomes; if a backstop
fires first, the run is reported as that backstop's code and flagged as a backend bug in `--verbose`.

## 7. Compiled-module cache

**R-SBX-13** Compiled modules are a **derived** cache (D-12) kept in a **user-level** directory, never inside the
project: `$XDG_CACHE_HOME/velme/wasm/<artifact-hash>-<key>.cwasm` (or the platform equivalent), where `key` = BLAKE3
of (`velme-wasm` version, Wasmtime version, engine config). They are never locked, never committed and may be deleted
at any time (D-48).
**R-SBX-14** `Module::deserialize` is used only on files under that user-level cache directory, written by this
process's engine configuration; a key mismatch or read error falls back to recompiling from IR. Files anywhere under
the project — including `.velme/` — are never passed to `Module::deserialize` (D-48, T-11): a `.cwasm` planted in a
cloned project is simply ignored and the goal is compiled fresh from its validated IR. CI must not restore this cache
across a trust boundary (e.g. from a fork's PR onto a shared runner).

## 8. Differential testing (INV-3)

**R-SBX-15** Every golden program and every IR in the fuzz/property corpus runs on both backends; the gate compares
result value (canonical JSON), outcome code, Velme fuel used and Velme memory used (cumulative bytes allocated,
D-53). Any difference fails the gate
(`delivery/51`).
**R-SBX-16** A new IR node kind or builtin is not released for the WASM backend until it passes the differential suite;
until then the emitter declines the goal and `auto` uses the interpreter.

## 9. Future: Component Model

Goals as WASM components with WIT interfaces (records/lists natural across languages), component composition for
composite goals, and WASI capabilities tied to declared `effects`. Tracked by RFC; must not weaken INV-3/INV-4.

## 10. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SBX-01 | Every golden leaf goal gives identical value, outcome code, fuel and memory used (cumulative bytes allocated) on `interp` and `wasm`. |
| AC-SBX-02 | A module importing a non-whitelisted function (e.g. `wasi_snapshot_preview1.fd_write`) fails with `VL0801` and never runs. |
| AC-SBX-03 | An expensive leaf is stopped with `VL0601` on WASM with the same fuel figure as the interpreter — AC-RDM-07. |
| AC-SBX-04 | A leaf that allocates past `max_memory` fails with `VL0604` on both backends at the same point. |
| AC-SBX-05 | Division by zero traps with `VL0602` on WASM; a module containing any `f32`/`f64` instruction is rejected by validation. |
| AC-SBX-06 | Every emitted module in the golden set validates with `wasmparser` under exactly the R-SBX-07 feature set. |
| AC-SBX-07 | Deleting the user-level `velme/wasm/` cache directory changes no result; the next run recompiles. |
| AC-SBX-08 | An epoch-deadline test (watchdog set to 1 ms) yields `VL0603` flagged non-reproducible. |
| AC-SBX-09 | A crafted `.cwasm` file planted anywhere under the project (including `.velme/`) is never passed to `Module::deserialize`; the goal compiles from IR instead and runs normally (D-48). |
