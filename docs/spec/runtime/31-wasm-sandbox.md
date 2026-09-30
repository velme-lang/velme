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
**R-SBX-02** Backend selection: `--backend interp|wasm|auto`, a flag only, with no `velme.toml` key (D-117). The default
is `interp` until the M7 gate passes, then `auto`; only the CLI's default flips, never the runtime's own (D-121). `auto`
runs a leaf goal on WASM when the sandbox can load and start it, and on the interpreter on any failure before its module
starts: the emitter declines it, or the sandbox can't be made, or can't compile, link or start the module. `--verbose`
then notes "`G` ran on the interpreter: …" with the reason (D-121). A leaf whose module has started is never run again
on the interpreter, whatever its outcome: a backstop firing is a backend bug (D-115). `wasm` does the same, except that
a leaf the WASM backend can't run is `VL0607` (`VL0801` for an import the sandbox refuses), never a silent fallback. On
a composite goal both run the leaves on WASM and the tail on the interpreter.
**R-SBX-17** Only leaf goal bodies in `run`, `test` and `trace` use WASM. Under `velme test` the leaf body of each
example and of each generated input runs on the selected backend, while the examples' expected-value expressions and
every check evaluate on the interpreter (D-80); a test run changes no artifact, lock or store, and only the derived
module cache (R-SBX-13) may be written. Call arguments, composite tails, build verification and `--locked`
re-verification stay on the interpreter, whatever `--backend` says (D-80, D-117).
**R-SBX-18** Output is byte-identical for all three values: stdout, the diagnostics on stderr, the exit code, `--json`,
and `trace --json` with its fuel and memory figures, for a success and for every deterministic failure, except a leaf
the WASM backend can't run under `wasm` (R-SBX-02), which is `VL0607` or `VL0801`. Timings and `VL0603` are excluded,
and `--verbose` notes are outside this rule. Waiting for one of the `MAX_WASM_RUNS` places counts against the run's wall
clock, so `VL0603` can come earlier on WASM. Neither `velme-cli/1` nor the trace has a `backend` field (D-117, D-121).

## 3. Value layout (ABI)

Chosen: a **type-directed in-memory layout** that both sides share. The host writes inputs directly into the module's
linear memory in this layout and reads the result back; the module never parses JSON or a tagged format.

| Type | Layout (little-endian, 8-byte aligned slots) |
|---|---|
| `Number` | 16-byte, 8-aligned slot: two `i64`, `lo` then `hi`. `lo` = magnitude bits 0..63; `hi` = magnitude bits 64..95 in its low 32 bits, the scale in bits 32..39, the sign in bit 63. One canonical form per value (D-113) |
| `Boolean` | `i32` 0/1 |
| `Nothing` | zero-size; a `List<Nothing>` is its length |
| `T?` | one 8-byte slot holding an `i32`: 0 = nothing, otherwise the address of the `T` slot, which for a record is the record itself. An absent `T?` takes the 8 bytes it is charged (30 §7.1, D-119) |
| `Text` | `i32 ptr, i32 len` → UTF-8 bytes |
| `List<T>` | `i32 ptr, i32 len` → `len` contiguous `T` slots. Every list, the empty one too, has its logical size (30 §7.1) as an `i64` in the 8 bytes before its items, so charging a value that holds the list never walks it, and `ptr` is never 0 (D-119) |
| record | its logical size (30 §7.1) as an `i64`, then its fields in declared order, each in its slot, so charging a value that holds the record reads one `i64` whatever the record's width (D-119) |

Why: the types are statically known on both sides, so tags and a decoder inside WASM are unnecessary; emitting
typed loads/stores is simpler than emitting a parser, and the encoding is canonical by construction (deterministic).

**R-SBX-03** Module exports exactly: `memory`, `velme_alloc(size: i32) -> i32`, `velme_run(input_ptr: i32) -> i32`
(pointer to the output's slot: a record's own address, and the result slot of R-SBX-19 for any other type), and three
mutable globals, `velme_fuel_left`, `velme_memory_left` and `velme_reason` (D-113, D-119). The first two are `i64`, read as unsigned, and `velme_reason` is an `i32`. The host sets the first two from the
run's limits before it calls `velme_run`, and reads all three afterwards,
also after a trap: fuel used, memory used and the trap reason come from them. Limits are never baked into a module, so
a module is a pure function of its IR. Allocation is a bump allocator; nothing is freed — each invocation gets a fresh
instance. `input_ptr` is the address of the inputs, each in its slot in declared order, in memory the host got from
`velme_alloc`, which charges nothing. The host writes the size prefix of every list and record among them (§3, D-119). The memory declares no maximum and grows as the allocator needs.
**R-SBX-19** Linear memory holds only (a) the values 30 §7.1 charges, each in no more than its charged size,
bump-allocated; (b) the invocation's inputs, written by the host; (c) literal data segments; (d) a fixed scratch stack
of `MAX_COLLECTION_NESTING + 1` frames, each sized for `MAX_LIST_SIZE` and never bump-allocated. A collection node
uses the frame at its nesting depth, so a `sort_by` key that holds another `filter` or `sort_by` has a frame of its
own. The frames hold sort order and keys, filter selection, `map` results until their list is charged, the result slot
and host-call marshalling. S, the size of
the whole stack, is one named constant. Nothing else takes memory that grows with fuel (D-90, D-113, D-119).
The stack starts at address 0, the literal data follows it and the bump allocator starts after that. Frame `d` is at
`d` × the frame size, and holds: at offset 0 an `i32` that is 0, or 1 + the index of the element whose lambda the
collection node at depth `d` is running, which the host reads innermost frame first after a trap to name the items of
the failure (language/14 R-BLT-07, D-118); at offset 16, 48 bytes for host-call marshalling; at offset 64 a collection
area of 24 bytes per item of a list of `MAX_LIST_SIZE`. The result slot is the start of frame 0's collection area: the
slot of an output that is not a record, with the `T` of a present `T?` right after it. A record is never in a frame — a
record output is returned by its own address (R-SBX-03) — so no record type is too wide for one (D-119).
**R-SBX-04** The host validates every pointer/length it reads back against memory bounds; an out-of-range result, or a
`Number` that is not in its canonical form (D-113), is `VL0607` (a backend bug, never user-visible data corruption).

## 4. Code generation & validation

**R-SBX-05** The emitter instruments Velme fuel and memory explicitly: each IR node takes its cost from the shared cost
table (30 R-RUN-04) off `velme_fuel_left`; each value creation takes the 30 §7.1 size off `velme_memory_left`, so
memory used is the cumulative bytes allocated (30 §7.1, D-53). Crossing a limit leaves that global at 0, sets
`velme_reason` and traps; the host maps the reason to `VL0601`/`VL0604` (D-119). A total of bytes that would pass
`u64::MAX` is past every limit, on both backends. The reasons are: 0, the module raised
nothing; 1, out of fuel (`VL0601`); 2, out of memory (`VL0604`); 3, a state the emitted code rules out (`VL0607`); 4,
the host refused to grow the memory (the `StoreLimits` backstop, R-SBX-12). One paid fuel unit covers at most a
constant number of emitted instructions, whatever the program's types (§6, D-115, D-119).
The cost table is one constant set in `velme-builtins`, depended on by both `velme-interp` and `velme-wasm` (D-54).
**R-SBX-06** `Number` arithmetic and comparison are host imports (`velme.num_add`, `num_sub`, `num_mul`, `num_div`,
`num_neg`, `num_cmp`) implemented by the same `velme-builtins` code as the interpreter, so results match bit for bit
(D-36). An import that fails — division by zero or overflow (`VL0602`), or an argument another import of §5 refuses
(`VL0602`, `VL0606`) — does not return: the host keeps the `velme-builtins` error, whose message names the operands, and
raises the trap itself, and `velme_reason` stays 0 (D-119). Numbers cross these imports, and
the other imports of §5, as values — two `i64`, `lo` then `hi` (§3), never through linear memory (D-90, D-112).
`==` and `!=` on two `Number`s compare their slots, since a value has one canonical form (D-113).
**R-SBX-07** The emitter uses only: MVP instructions except any `f32`/`f64` operation (`Number` is decimal, D-36), multi-value, bulk
memory, mutable globals. No threads, SIMD,
relaxed SIMD, reference types, exceptions or tail calls in v0.1. `wasmparser` validates every module with exactly this
feature set before it is cached or instantiated.
**R-SBX-08** Host imports, under module `velme`, do scalar work only: each does a constant amount of host work per
call, so Velme fuel stays a true bound on the run (D-112). They are the imports of §5, implemented by `velme-builtins` —
the same Rust code the interpreter calls — so their semantics cannot diverge. Everything that walks a `List` or a
`Text` is emitted WASM: the list and text builtins, the character count of `length` included, equality on composite
values, and the collection nodes (`map`,
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
| `velme.num_cmp` | `(n, n) -> i32` | yes | the order of the two numbers: exactly -1, 0 or 1 (D-119) |
| `velme.abs`, `floor`, `ceil`, `round` | `(n) -> n` | yes | language/14 |
| `velme.clamp` | `(n, n, n) -> n` | yes | `x`, `low`, `high`; `low > high` is `VL0602` |
| `velme.random` | `(n, n) -> n` | yes | `seed`, `index` (D-22) |
| `velme.to_text` | `(n, ptr: i32) -> i32` | yes | writes the text, at most 48 bytes, into the scratch frame at `ptr`, returns its byte length |
| `velme.range_len` | `(n) -> i32` | yes | checks the argument of `range` (`VL0602`, `VL0606`) and returns the length; the list is built by emitted code |
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
| `consume_fuel` | on, set to `max_fuel × K + A + M × m`, saturating, where m is the run's `StoreLimits` memory figure: K is 4 × the most instructions one paid Velme fuel unit covers, A is 4 × what runs unpaid (the one `sum` paid after its work, `velme_alloc` and storing the result), and M is 4 × the most times emitted code moves one charged byte with a bulk copy; all three are named constants of `velme-wasm`. Wasmtime's own costs are kept: one unit an instruction, and one a byte or page that bulk memory or growth moves, so Cranelift checks fuel and the epoch after each such instruction | backstop only; Velme fuel (R-SBX-05) is the deterministic limit, and a legal program never reaches the backstop (D-115, D-120) |
| `epoch_interruption` | on; ticker thread increments every 10 ms; the deadline is one tick, and at each deadline a callback asks the run's shared watchdog (30 §7), which reads the injected clock: it either allows one more tick or stops the module. The first deadline is the call of `velme_run` itself, so a run that starts past its time is stopped before it runs | wall-clock safety net → `VL0603` (D-10, D-51, D-115); tests drive it with the fake clock |
| Wasm features | every feature off, then exactly the R-SBX-07 set on: threads, SIMD, floats, sign extension and saturating conversions are off in Wasmtime too | determinism (R-SBX-07, D-120) |
| `StoreLimits` | memory ≤ `max_memory` + the ABI bytes of the invocation's inputs + the data-segment bytes + S (R-SBX-19) + a fixed overhead of 1 MiB, rounded up to 64 KiB pages, and under 4 GiB; 1 memory, no table, 1 instance | host-side backstop behind the deterministic memory counter → `VL0604` (D-53, D-90, D-113, D-120) |
| Memory reservation | the `StoreLimits` memory figure at the system cap `max_memory`, one figure for the engine, not Wasmtime's 4 GiB default; memory may move, so a run whose inputs or literals take it past that grows by moving. The runtime's limits are always within the system caps | no address space beyond what a run may use (D-113, D-120) |
| WASM stack | 4 MiB, emitted code never recursing; a module runs on a thread of its own with a 6 MiB stack, room for that and the host's calls. A panic of the host on that thread goes on in the caller's, never as `VL0607` | fixed, so deep emitted code fails the same way on every host (D-113, D-120) |
| `wasmtime` crate | default features off; only `runtime`, `cranelift` and `std`; always built in, with no cargo feature to turn it off | small dependency surface; one build of `velme` (D-116) |
| Instantiation | `InstancePre` per module, fresh `Store` + instance per invocation | no state shared between calls |

**R-SBX-11** A Wasmtime trap is classified in this order, the first that applies (D-119): (1) a failure an import kept
(R-SBX-06), carried as the trap's own error payload → its `VL0602`/`VL0606`; (2) the watchdog's interrupt (the epoch
deadline) → `VL0603`; (3) Wasmtime out of fuel → the backstop `VL0601`; (4) `unreachable` with `velme_reason` 1 →
`VL0601`, 2 → `VL0604`, 3 → `VL0607`, 4 (`ResourceLimiter` denial) → the backstop `VL0604`; (5) anything else — a
stack overflow, an out-of-bounds access, `unreachable` with reason 0 → `VL0607 InternalError` (and is a bug).
The host also reports `VL0607` for: a `left` global above the limit it set; a reason outside 0..4; a non-zero reason
after a normal return; reason 1 or 2 with its global not 0; a visiting index, or a list length, above `max_list_size`.
Decoding the result stops, as `VL0607`, once it passes either of two budgets (D-120). Bytes: the logical size decoded
passes `max_memory` plus what no run is charged for: the logical size of the inputs, the sum of the logical sizes of the
goal's literals as the emitter counts them, and 64 bytes for a scalar output (D-53, D-83, D-89). Items: the items of the
lists of `Nothing` the host makes, which have no size, pass the fuel spent plus the items of the inputs' and the
literals' lists of `Nothing`; a list of a length already made is shared and takes nothing. The `to_text` import refuses
a text longer than the 48 marshalling bytes and a `ptr` that is not the marshalling bytes of a scratch frame; every
`Number` that crosses the boundary, in either direction, goes through the canonical-form check of R-SBX-04.
**R-SBX-12** Only the deterministic limits (Velme fuel, Velme memory) can produce reproducible outcomes; if a backstop
fires first, including host resource exhaustion (memory growth refused by the OS after start), the run is reported as
that backstop's code. `--verbose` adds a note on stderr saying that a backstop fired, which is a backend bug; nothing
else changes, in `--json` or in the trace (D-115).

## 7. Compiled-module cache

**R-SBX-13** Compiled modules are a **derived** cache (D-12) kept in a **user-level** directory, never inside the
project: `$XDG_CACHE_HOME/velme/wasm/<module-hash>-<compat-hash>.cwasm`, else `~/.cache/velme/wasm/…` by the XDG rule on
Linux and macOS alike, where `module-hash` = BLAKE3 of the emitted module's bytes and `compat-hash` is Wasmtime's
compatibility hash for the engine, so a change to the emitter, to Wasmtime or to the engine configuration gives a new
name (D-116). Emission always runs; only Wasmtime's compiled output is cached. They are never locked, never committed
and may be deleted at any time (D-48).
**R-SBX-20** The cache directory is an option passed to the backend: `velme-cli` passes the user-level directory, and a
test passes a temporary directory or none (no disk cache). The directory is created with mode `0700`, then opened
without following a symbolic link, and checked on that handle: a directory owned by the process's effective user, with
no access for group or others. One that fails is refused and never changed. Every file is reached through that handle:
read only if, opened without following a link, it is a regular file of the same owner that group and others cannot
write; written under a temporary name, mode `0600`, and renamed into place atomically. The runtime also refuses a
directory that is not absolute, or that lies inside the project once both are resolved (T-11). A refusal turns the disk
cache off for the process: modules are compiled on every run, and `--verbose` says why. There is no disk cache off Unix
in v0.1 (D-116, D-120).
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
until then the emitter declines the goal and, as for any leaf the sandbox can't load and start, `auto` uses the
interpreter and an explicit `wasm` is `VL0607` (R-SBX-02, D-121).

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
| AC-SBX-07 | Deleting the WASM cache directory changes no result; the next run recompiles, seen as the cache file appearing again. Tested with a temporary cache directory (R-SBX-20), on Unix; elsewhere there is no disk cache in v0.1 and nothing is written (D-120). |
| AC-SBX-08 | With the injected clock past `max_wall_clock`, a run on WASM ends with `VL0603`, flagged non-reproducible (D-115). |
| AC-SBX-09 | A garbage `.cwasm` file planted under the project's `.velme/` is never read: the goal compiles from IR and runs normally, and the loader refuses any path outside the configured cache directory (D-48, D-116). |
