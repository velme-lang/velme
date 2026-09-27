# 30 — Execution: VibeVM & Reference Interpreter

**Status:** v0.1 · **Area:** RUN
**Read when:** working on the interpreter, the composite-goal scheduler, budgets, failure handling, determinism, traces or `velme explain`.
**Depends on:** [SPEC](../SPEC.md), [compiler/21](../compiler/21-ir.md), [language/12](../language/12-goals-calls.md), [language/13](../language/13-check-dsl.md), [32-artifacts-cache](32-artifacts-cache.md)
**Source:** §3.4, §9–§11, §24, §28 (Calls), §29, §30, §31, §33, §34, §39

## 1. Purpose & boundaries

VibeVM runs verified IR artifacts: it plans a composite goal's calls as a DAG, runs independent calls concurrently,
enforces budgets, evaluates checks and records a deterministic trace. The **reference interpreter** in `velme-interp`
defines what IR means (P-4); the WASM backend ([31](31-wasm-sandbox.md)) must agree with it (INV-3). Artifact
identity and loading are in [32](32-artifacts-cache.md).

## 2. Components (§24.1)

| Component | Responsibility | Crate |
|---|---|---|
| GoalRegistry | goal id → signature, kind, artifact hash (from `velme.lock`), dependencies (§24.2 fields, minus hashes now held by the manifest, 32 §3) | `velme-runtime` |
| ArtifactStore | load + re-validate artifacts by hash (32) | `velme-runtime` |
| CallPlanner | per composite goal: binding DAG → waves (static, from HIR/IR `args`) | `velme-runtime` |
| Scheduler | runs ready bindings on a bounded worker pool; applies D-9 on failure | `velme-runtime` |
| Executor | evaluates one goal body: interpreter (default) or WASM (M7) | `velme-interp`, `velme-wasm` |
| CheckRunner | evaluates lowered checks on every run (D-20) | `velme-check` |
| BudgetManager | static call/depth limits, per-invocation fuel/memory/size, wall-clock watchdog | `velme-runtime` |
| Trace | ordered execution record; source of `velme trace`, debugging and telemetry | `velme-runtime` |

## 3. Interpreter semantics (reference)

**R-RUN-01** Values: `Number` (exact decimal, D-36), `Text` (UTF-8), `Boolean`, `Nothing`, `List` (immutable), `Record` (fields
in declared order). Values are immutable and structurally compared.
**R-RUN-02** Evaluation is strict and left-to-right: `binary` left then right (except short-circuit `and`/`or`),
record fields in declared order, list items in order, `let` binds in order, collection lambdas over elements in list
order. Evaluation order is observable only through which error is reported first, and it is fixed.
**R-RUN-03** Arithmetic follows `language/11` R-TYP-04..06: exact decimal, half-to-even rounding; division by zero or
overflow is `VL0602`; `-0` results are normalized to `0` (D-36).
**R-RUN-04** Every IR node evaluation costs 1 fuel unit; each element visited by
`map`/`filter`/`find`/`reduce`/`all`/`any` costs 1 more; `==`/`!=` on a `List`/`Record` value costs 1 + the scalar
leaves compared, stopping at the first difference; `sort_by` and every value/text builtin cost what their catalog
entry says (language/14 §2, §4, D-52). This **Velme fuel** cost model is part of the semantics — both backends
meter it identically (31 R-SBX-05).
**R-RUN-05** No I/O, clock, randomness or global state is reachable from the interpreter (INV-4); `random` is a pure
builtin of its arguments (D-22).

## 4. Composite goal execution (§29)

```
run(goal, input)
  1  look up artifact via lock → load + validate (32)          fail → Unavailable / ValidationFailed
  2  validate input against the signature (D-23)               fail → VL0902
  3  build waves from `calls` args (R-IR-10)
  4  for each wave: run all its bindings concurrently (each = recursive run of the child)
  5  collect results in source order; on failure apply D-9
  6  evaluate the tail `body` with inputs + bindings as locals
  7  run checks on (inputs, bindings, result)                  fail → VL0501
  8  enforce output size; return value + trace
```

**R-RUN-06** Waves are computed statically: a binding's wave is 1 + the max wave of the bindings its args reference
(inputs are wave 0). Source order never implies execution order (§9); the learner-visible model is the wave list.
**R-RUN-07** Concurrency runs on Tokio; interpreter evaluation (CPU-bound) runs on a bounded blocking pool sized by
`--jobs` (default: available CPUs). Results are identical for every `--jobs` value, including 1 (INV-3).
**R-RUN-08** No memoization across calls in v0.1: the same child called twice with equal arguments runs twice (keeps
call counts and traces simple and static).

Worked example (§11) — `CreateLevelSummary`:

| Wave | Bindings |
|---|---|
| 1 | `enemies = FindEnemies(level)`, `treasures = FindTreasures(level)`, `score = CalculateScore(player)` |
| 2 | `difficulty = EstimateDifficulty(enemies)`, `reward = CalculateReward(treasures, score)` |
| tail | `CreateLevelSummary` body over all five bindings |

## 5. Failure semantics (§30)

| Outcome | Meaning | Codes |
|---|---|---|
| `Success(value)` | body and checks passed | — |
| `Failure(error)` | the goal ran and failed | `VL0501`, `VL0602`, `VL0902`, `VL0607` |
| `BudgetExceeded` | a deterministic limit was hit | `VL0601` fuel, `VL0604` memory, `VL0605` calls, `VL0606` size |
| `Timeout` | wall-clock watchdog fired — **non-reproducible** (D-10) | `VL0603` |
| `ValidationFailed` | a loaded artifact failed re-validation or its hash | `VL0402`, `VL0703` |
| `Unavailable` | no artifact for the goal (not built / not locked) | `VL0701` |

**R-RUN-09** A required child failure fails the parent (§30), but every sibling in that binding's wave still runs to
completion — no cancellation — and no later wave starts (D-9). The reported failure is that of the **lowest
source-order** binding that failed among the wave(s) that ran; every other failure in those waves is listed as a note,
in source order. A binding in a wave that never started appears in the trace as `skipped`, not `cancelled`.
**R-RUN-10** The parent's failure wraps the child's: `BuildPlayerSummary failed because FindBadge failed: …`, keeping the
child's code as the root cause. The top-level exit status uses the root cause's code (tooling/40).
**R-RUN-11** `fallback`, `retry` and optional calls are Future (reserved, D-24).

## 6. Determinism (§31, INV-3)

**R-RUN-12** Same source + input + locked artifacts + seed ⇒ same result value, same outcome code and same trace
(modulo timing fields) — across runs, `--jobs` values, OSes and backends.
**R-RUN-13** Pure goals have no implicit clock, network, environment or randomness; seeds are explicit inputs (D-22).
**R-RUN-14** Only `Timeout` may differ between runs; it is flagged `reproducible: false` in the trace and is never
cached or used as a verification verdict (D-10).
**R-RUN-15** Failure messages render values with the canonical number format and sorted-by-declaration record fields,
so diagnostic text is itself deterministic.

## 7. Budgets (§3.4, §28, D-8)

| Field | Default (system cap) | Scope | Checked | Code |
|---|---|---|---|---|
| `max_fuel` | 10 000 000 Velme fuel | per goal invocation | runtime, deterministic | `VL0601` |
| `max_memory` | 64 MiB | per goal invocation (value bytes, §7.1) | runtime, deterministic | `VL0604` |
| `max_goal_calls` | 128 | whole run tree | **statically** at `velme check` (call graph is static) | `VL0605` |
| `max_call_depth` | 32 | whole run tree | statically | `VL0605` |
| `max_list_size` | 10 000 items | any list value | runtime | `VL0606` |
| `max_output_bytes` | 1 MiB | canonical JSON of a goal's result | runtime | `VL0606` |
| `max_wall_clock` | 60 s | whole top-level run | watchdog, safety net only (D-51) | `VL0603` |

**R-RUN-16** A goal's `budget` line (language/12) can only tighten these caps for that goal's own invocation:
`cpu=Nms` → `max_fuel = N × 100 000` (fixed conversion constant `FUEL_PER_MS`, not measured, so deterministic);
`memory=` → `max_memory`; `calls=`/`depth=` → limits for the subtree rooted at that goal. A value above the system cap
is rejected by `velme check` with `VL0308 InvalidBudget` (language/12 R-GOAL-20).
**R-RUN-17** Fuel and memory are per invocation, not a shared pool, so concurrent siblings cannot affect each other's
outcome (INV-3). The whole tree is still bounded: ≤ 128 invocations × per-invocation limits, plus the watchdog.
**R-RUN-18** Budget constants are defined once in `velme-runtime` and referenced by name by the CLI, the prompt
builder (22 §4) and tests.
**R-RUN-24** The static bound of R-RUN-17 (at most `max_goal_calls` × `max_fuel` = 1.28 × 10⁹ fuel, about 12.8 s at
`FUEL_PER_MS`) is the whole run's deterministic limit; `max_wall_clock` sits well above it and guards only against a
Velme bug or an overloaded host, never a verification verdict (D-51). Its clock is injectable, so tests drive it with
a fake clock instead of real time (R-QA-02).

### 7.1 Memory accounting

The interpreter charges each value it creates by a fixed size function (Number 16, Boolean 8, Nothing 0, `T?` 8 +
`T`, Text 16 + bytes, List 16 + Σ items, Record 16 + Σ fields — never less than the value's bytes in the WASM ABI's
8-byte-aligned slots, 31 §3, so the WASM memory backstop can't fire first) and tracks
the **cumulative bytes allocated** during the invocation, not the peak of live bytes (D-53). The function is part
of the semantics; the WASM backend charges the same numbers (31 R-SBX-05), not its linear-memory size. Building a
value one item at a time with `reduce` + `concat` therefore allocates `O(n²)` bytes; the synthesis prompt steers
plans toward `map`/`filter`/`range` instead (compiler/22 §4).

## 8. Trace (§33)

A trace is a tree of events, ordered by **source order**, never by completion time:

| Event | Fields |
|---|---|
| `goal` | goal, artifact hash, kind, inputs, outcome, fuel used, memory used (cumulative bytes allocated, §7.1), duration (non-deterministic, excluded from comparisons) |
| `call` | binding, child goal, wave, args, outcome (`ok` / `failed` / `skipped`), value, nested `goal` |
| `check` | source text, span, passed, values of every sub-expression referenced in a failure |
| `failure` | code, message, root-cause path (`BuildPlayerSummary › FindBadge`) |

**R-RUN-19** Trace JSON is versioned and additive-only; `velme trace --json` emits it; `velme run` renders the human
view on failure (§33 layout: each call with ✓/✗ and value, then the failed check with expected vs received).
**R-RUN-20** Values in the human view are truncated (lists > 10 items, text > 80 chars) with a count of what was
elided; the JSON form is complete up to `max_output_bytes`.
**R-RUN-21** Traces are local. Telemetry is a local aggregation of traces (counts, durations); nothing is sent
anywhere in v0.1 (tooling/41).

## 9. Explain mode (§34)

**R-RUN-22** `velme explain` renders a goal from its waves, deterministically and without an LLM:
one binding in a wave → "First: …" / "Then: …"; several → "At the same time: …"; the tail → "Finally: " + the plan's
first sentence. Binding lines use the child's plan first sentence, or "run `Child`" if it has none.
**R-RUN-23** Correction to §34: all three bindings of `BuildPlayerSummary` are in wave 1, so the explanation is a
single "At the same time:" group, not "First … At the same time …".

## 10. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-RUN-01 | Three independent children with equal heavy workloads are all in wave 1 and their executions overlap in time with `--jobs 3` — §52 Test 3. |
| AC-RUN-02 | `Main` → `Double` then `AddOne` runs in 2 waves and returns `2x + 1` — §52 Test 2. |
| AC-RUN-03 | Two siblings both fail: the reported failure is always the lower source-order binding, and both siblings' real outcomes (not `skipped`) appear in the trace, across 100 runs with random scheduling delays (D-9). |
| AC-RUN-04 | Result and trace (excluding durations) are byte-identical for `--jobs 1` and `--jobs 8` on the golden programs, including two failing siblings where the slower one is first in source order (D-9). |
| AC-RUN-05 | An over-budget leaf (a `reduce` over `range(10000)` whose lambda reduces over `range(10000)`) fails with `VL0601` deterministically, with the same fuel figure every run — §52 Test 7. |
| AC-RUN-06 | A call tree needing 129 invocations is rejected by `velme check` with `VL0605` before any execution. |
| AC-RUN-07 | A failing check reports the assertion text, the expected and received values — §52 Test 6. |
| AC-RUN-08 | `x / 0` in a leaf yields `VL0602`; no partial or special value appears in any output. |
| AC-RUN-09 | `budget cpu=1ms` on a goal whose run needs more than 100 000 fuel yields `VL0601`; the same goal without the line succeeds. |
| AC-RUN-10 | `velme explain` output for `BuildPlayerSummary` and `CreateLevelSummary` matches the golden text; no provider is constructed. |
| AC-RUN-11 | A wall-clock timeout produces `VL0603` with `reproducible: false` in the trace and no artifact or cache write. |
| AC-RUN-12 | With an injected fake clock, a run is stopped with `VL0603` (`reproducible: false`) only once the clock passes 60 s; a run of 128 invocations each using its full `max_fuel` under a clock advancing at `FUEL_PER_MS` completes without `VL0603` (D-51). |
