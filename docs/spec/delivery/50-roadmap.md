# 50 — Roadmap, MVP Scope & Success Criteria

**Status:** v0.1 · **Area:** RDM
**Read when:** deciding what to build next, whether something is in the MVP, or what "done" means for v0.1.
**Depends on:** [SPEC](../SPEC.md), [51-testing-quality](51-testing-quality.md), all language/compiler/runtime specs (each owns its `AC-*`)
**Source:** §37, §38, §39, §51, §52, §53, §54, §55

## 1. Purpose & boundaries

Sequences the v0.1 build into phases M0–M8 (D-16), fixes the MVP scope and exclusions, and defines the MVP success
criteria. The working tracker (status, dates, gate log) lives in `docs/plan/mvp-plan.md`; this file is the stable
definition it tracks.

**R-RDM-01** Build only the current phase. Don't scaffold later phases; `Future` items are never built in v0.1.
**R-RDM-02** Each phase ends at a Stop & Verify Gate with its exit criteria green and its `AC-*` ids covered by
tests (R-QA-01), before the next phase starts.
**R-RDM-03** Every phase is testable without the phases after it: before M5 there is no LLM — IR is hand-written in
fixtures; before M7 the interpreter is the only backend.

## 2. Phases

Order differs from the original §51 so leaf execution exists before the DAG runtime (F-15, D-16).

| Phase | Goal | Scope | Specs touched | Deliverable | Exit criteria |
|---|---|---|---|---|---|
| **M0** | Repository & gate | Cargo workspace with the D-15 crates as empty libs, pinned toolchain, `cargo xtask verify`, CI skeleton, license files, `README`, `CONTRIBUTING.md`, `AGENTS.md`, `CLAUDE.md` (D-40) | `delivery/51`, `delivery/52` | `cargo xtask verify` green | AC-REL-01, AC-REL-02, AC-QA-01 |
| **M1** | Syntax | indentation-aware lexer, Chumsky parser for the whole v0.1 grammar incl. literals, `examples`, `budget`, header; AST with spans; diagnostics crate + ariadne rendering; parser golden corpus | `language/10`, `compiler/20`, `reference/90` | `thela check` reports parse errors | AC-SYN-*, `TL01xx` golden tests, AC-CLI-01 (parse part) |
| **M2** | Semantics | name resolution, type checking incl. check DSL narrowing, call rules, cycle detection, wired goals (D-4), JSON input mapping | `language/11..14`, `compiler/20` | `thela check` fully validates a program | AC-TYP-*, AC-GOAL-* (static), AC-CHK-* (typing), AC-RDM-04, AC-RDM-05 |
| **M3** | IR, interpreter, checks | IR types + JSON Schema + validator; builtins incl. `random`/`range`; reference interpreter for leaf goals; check and example evaluation; hand-written IR fixtures | `compiler/21`, `language/13..14`, `runtime/30` | `thela test` on a leaf goal with fixture IR | AC-IR-*, AC-BLT-*, AC-CHK-* (eval), AC-RDM-06 |
| **M4** | DAG runtime | composite goals, wave scheduling on Tokio, deterministic failure (D-9), budgets (fuel in interpreter, calls, depth, sizes), trace, `run`/`trace`/`explain` | `runtime/30`, `tooling/40` | `thela run multi_goal.thela` | AC-RUN-*, AC-RDM-02, AC-RDM-03, AC-CLI-06/07 |
| **M5** | Spellbook | `SynthProvider` trait, `anthropic`/`ollama`/`external`/`replay`/`scripted`, external protocol, prompt contract, structured output, retry with diagnostics, verification pipeline, generated test inputs | `compiler/22`, `tooling/41` | plan → runnable goal via `thela build` | AC-SYNTH-*, AC-SEC-02/03/05/06, AC-RDM-01 |
| **M6** | Artifacts & CLI | fingerprints (D-11, D-21), content-addressed store, `thela.lock`, `--locked`/`--offline`, `artifact` command, config file | `runtime/32`, `tooling/40` | warm cached run with zero synthesis | AC-ART-*, AC-CLI-*, AC-RDM-08, AC-RDM-09 |
| **M7** | WASM backend | IR → core WASM for leaf goals, `wasmparser` validation, Wasmtime with fuel + epoch + `ResourceLimiter`, host-function allowlist, differential tests vs interpreter | `runtime/31`, `delivery/51` | `thela run --backend wasm` equals interpreter | AC-SBX-*, AC-SEC-01/07, AC-RDM-07 |
| **M8** | MVP gate | all success criteria end-to-end, examples tree, fuzz smoke, perf targets, security baseline, docs, release dry run | all | `v0.1.0-alpha` release candidate | §5 all green; `delivery/51` §6 targets met |

## 3. MVP scope (§37)

| Area | Item | Phase | Status |
|---|---|---|---|
| Language | `type`, `goal`, `plan`, `check`, `call` | M1–M2 | v0.1 |
| Language | `List<T>`, `T?`, `Number`/`Text`/`Boolean`/`Nothing`, records | M1–M2 | v0.1 |
| Language | `examples:` (D-7), `budget` (D-8), `language:` header | M1–M3 | v0.1 |
| Compiler | parser, AST, symbol resolution, type checking | M1–M2 | v0.1 |
| Compiler | call graph, cycle detection | M2 | v0.1 |
| Compiler | Thela IR, IR validation | M3 | v0.1 |
| AI | real providers `anthropic` + `ollama` (D-41), structured IR output, retry with diagnostics | M5 | v0.1 |
| AI | `external` backend protocol for human/tool-written IR (D-42) | M5 | v0.1 |
| AI | replay + scripted providers (D-13) | M5 | v0.1 |
| Runtime | reference interpreter | M3 | v0.1 |
| Runtime | composite-goal DAG executor, parallel waves | M4 | v0.1 |
| Runtime | CPU (fuel), memory, call, depth, size budgets | M4, M7 | v0.1 |
| Runtime | WASM backend for leaf goals, Wasmtime sandbox | M7 | v0.1 |
| Validation | deterministic check DSL, generated test values, check runner | M3, M5 | v0.1 |
| Validation | artifact cache + lockfile (D-12) | M6 | v0.1 |
| DevEx | CLI, source errors, execution trace, IR view (`artifact`), local telemetry | M1–M6 | v0.1 |

## 4. Not in the MVP (§38)

| Excluded | Status | Note |
|---|---|---|
| Recursion (self or mutual) | Future | rejected `TL0304` |
| Loops, `when`/`choose` conditional calls | Future | keywords reserved (D-24) |
| Networking, filesystem, databases inside goals | Future | INV-4 |
| User-defined effects | Future | `effects` reserved |
| Automatic production hot swap | Future | Coach, `runtime/32` |
| Multiple LLM agents, fine-tuning | Future | provider trait allows later |
| Package ecosystem, modules/imports | Future | D-18 single file; design: Q-16 |
| Full Component Model composition / WIT | Future | `runtime/31` |
| Distributed execution, remote artifact store | Future | `runtime/32` |
| Mathematical proof of checks | Future | checks are properties, not proofs |
| Maps, enums, tuples, generics, binary `Float` | Future | D-36 |
| `Date`, `DateTime`, `Instant`, `Duration` | Future | design: Q-18 |
| Untyped beginner parameters | Future | D-3 |

## 5. MVP success criteria (§52)

**R-RDM-04** The MVP gate passes only when each criterion below passes reliably: 20 consecutive runs of its test in
the replay/scripted configuration, plus one recorded live run per synthesis criterion.

| ID | Criterion | Program / check | Phase |
|---|---|---|---|
| AC-RDM-01 | **Simple goal** — synthesized from plan, passes its check | `goal Add(a: Number, b: Number) -> Number:` `plan: "Add a and b."` `check: - result == a + b` | M5 |
| AC-RDM-02 | **Goal composition** — wired composite goal runs with no synthesis for `Main` | see below | M4 |
| AC-RDM-03 | **Parallel calls** — three independent children run concurrently (overlapping spans in trace), result and trace order identical to sequential execution | `BuildPlayerSummary` 3-call form | M4 |
| AC-RDM-04 | **Type mismatch** — invalid call rejected before execution | `CalculateScore("hello")` → `TL0204` | M2 |
| AC-RDM-05 | **Cycle** — `A → B → A` fails compilation | → `TL0304` naming the cycle | M2 |
| AC-RDM-06 | **Check failure** — shows the exact failed assertion and the values | → `TL0501` with expected/got | M3 |
| AC-RDM-07 | **Timeout** — deliberately expensive program terminated | fuel → `TL0601`; watchdog → `TL0603` | M4 (interp), M7 (WASM) |
| AC-RDM-08 | **Cache** — unchanged source performs zero synthesis | provider call count = 0 on second `build` | M6 |
| AC-RDM-09 | **Reproducibility** — same source + inputs + lock + seed ⇒ byte-identical result and trace, interpreter and WASM | 100 runs, both backends | M6, M7 |

AC-RDM-02 program (legal per D-4 — every goal has a body):

```text
goal Double(x: Number) -> Number:
    plan: "Multiply x by 2."
    check:
        - result == x * 2

goal AddOne(x: Number) -> Number:
    plan: "Add 1 to x."
    check:
        - result == x + 1

goal Main(x: Number) -> Number:
    call:
        doubled = Double(x)
        result = AddOne(doubled)
    plan: |
        Return the result from the second goal.
```

## 6. After the MVP

| Item | Source | Notes |
|---|---|---|
| Playground: editor, run button, trace view, goal graph, friendly failures | §51 Phase 5 | consumes library APIs (INV-9); first place D-37 applies |
| Coach: telemetry → candidate → validate → checks → benchmark → store; later A/B, promotion, rollback | §35, §36, §51 Phase 6 | safety rule in `runtime/32` |
| Tree-sitter grammar, LSP, VS Code extension | §19, §43.2 | D-25 corpus parity |
| Second provider, provider routing | §42 | trait already neutral |
| Component Model + WIT, typed cross-goal components | §40, §41 | v0.2+ |
| Conditional calls, effects, modules | §13, §49 | each through an RFC (`delivery/52`) |
| Remote artifact store (Redis + object storage + PostgreSQL), Thela Cloud | §26, §42A.4 | separate private repos |

## 7. Acceptance criteria

This file's criteria are AC-RDM-01..09 in §5.
