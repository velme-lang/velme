# Velme — MVP Implementation Plan

**Goal:** pass the MVP success criteria `AC-RDM-01..09` (`delivery/50`), each backed by the `AC-*` tests of its owning
spec file, with the quality gate (`delivery/51`) green.
**Read when:** starting a phase, finishing a phase, or checking what's next. Read **§1 Progress tracker** and **only the
phase you are working on** — each phase lists the spec sections it needs.
**Workflow:** `AGENTS.md` (plan → small steps → Stop & Verify Gate → explicit approval → next phase).

---

## 1. Progress tracker

Status values: `TODO` · `IN PROGRESS` · `GATE` (waiting for user verification) · `DONE` · `BLOCKED`.
Update the row and the gate log **at every Stop & Verify Gate**; don't rewrite phase bodies to record progress.
"Model" is the tier and effort that build the phase (set with `/model` and `/effort` for inline work); "→ Opus review"
means an `architect-review` (Opus, high) runs before the phase gate.

| Phase | Name | Model | Status | Started | Verified | Notes |
|---|---|---|---|---|---|---|
| M0 | Workspace, conventions, verify gate | Sonnet · medium | DONE | 2026-09-25 | 2026-09-25 | |
| M1 | Syntax: lexer, parser, AST, diagnostics | Opus · high | TODO | | | grammar is a language decision |
| M2 | Semantics: names, types, call graph | Opus · high | TODO | | | |
| M3 | IR, validator, interpreter, check evaluator, store + lock | Opus · high | TODO | | | defines reference semantics (P-4) |
| M4 | VibeVM: DAG scheduler, budgets, trace, explain | Sonnet · medium → Opus review | TODO | | | determinism review (D-9, D-10) |
| M5 | Spellbook: providers, prompt, retry, verification | Sonnet · medium → Opus review | TODO | | | security review (prompt injection, secrets, external command) |
| M6 | CLI completion: locked/offline, artifact, config | Sonnet · medium | TODO | | | |
| M7 | WASM backend + Wasmtime sandbox | Opus · high | TODO | | | sandbox is security-critical |
| M8 | MVP gate: success criteria, examples, docs | Opus · medium | TODO | | | |

### Open questions

None open — Q-1..Q-7 resolved 2026-09-25 (`reference/92` §4 → D-36..D-39). Name screened and set to **Velme**
(D-38); the official trademark search is owed before public launch, not before M0.

### MVP gate checklist (`delivery/50` success criteria)

| # | Criterion | Delivered in | Evidence | ✓ |
|---|---|---|---|---|
| 1 | Simple goal synthesizes, verifies and runs | M5 | AC-RDM-01 (replay fixture) + one live run by hand | ☐ |
| 2 | Goal composition (wired + synthesized tail) | M2, M4 | AC-RDM-02 | ☐ |
| 3 | Independent calls execute concurrently | M4 | AC-RDM-03 | ☐ |
| 4 | Type mismatch caught before execution | M2 | AC-RDM-04 | ☐ |
| 5 | `A → B → A` fails compilation | M2 | AC-RDM-05 | ☐ |
| 6 | Failed check shows assertion and values | M3, M4 | AC-RDM-06 | ☐ |
| 7 | Expensive program is terminated | M4, M7 | AC-RDM-07 (fuel, both backends) | ☐ |
| 8 | Unchanged source never re-synthesizes | M5 | AC-RDM-08 (provider call count = 0) | ☐ |
| 9 | Reproducible: same source+inputs+lock+seed ⇒ same result | M4, M7 | AC-RDM-09 (interp + WASM, repeated) | ☐ |

### Gate log

Append one line per approved gate: `YYYY-MM-DD · Mn · approved by <who> · commit <sha> · AC covered / deferred`.

---

## 2. Scope

**In the MVP:** everything tagged `v0.1` in the spec — the `delivery/50` must-have list. One file per program, the
`anthropic`, `ollama` and `external` providers (D-41, D-42), local artifact store + `velme.lock`, CLI only.

**Not in the MVP** (`delivery/50` NOT-MVP list): recursion, loops, conditional calls, effects, modules, playground,
Coach, Tree-sitter/VS Code, Component Model, cloud storage, further providers. Reserved keywords (D-24) are lexed and
rejected with `VL0104`; nothing else is scaffolded.

---

## 3. Phases

Each phase: read the listed spec sections only. A phase with slices is built slice by slice (`AGENTS.md` §Slices).

### M0 — Workspace, conventions, verify gate

**Before:** name screened (D-38) ✓; claim `velme` and the `velme-*` crates on crates.io
when the workspace is created (`delivery/52` §11).
**Read:** `delivery/52` (layout, CI), `delivery/51` (gate), `docs/code-conventions.md`.
**Build:** Cargo workspace with the D-15 crates as empty libs (+ `velme-cli` bin printing version), `rust-toolchain.toml`,
`rustfmt.toml`, workspace lint table (CC-ERR-01, CC-API-04), `deny.toml`, `xtask` with `verify` (fmt, clippy, test,
deny, crate-dependency check for INV-9, AC coverage audit stub), `insta` wired, README/LICENSE-MIT/LICENSE-APACHE/LICENSE-CC-BY/CONTRIBUTING/SECURITY
per `delivery/52` first-commit checklist, `examples/` skeleton.
**Exit:** `cargo xtask verify` green (AC-REL-01/02, AC-QA-01/03, AC-CMP-01); the dependency check fails when a forbidden
edge is added (demonstrated in a test).
**User verifies:** `cargo xtask verify`; `cargo run -p velme-cli -- --version`.

### M1 — Syntax

**Read:** `language/10` (all), `reference/90` §codes VL01xx, `compiler/20` §diagnostics.
**Slices:** M1a lexer + indentation (INDENT/DEDENT, block scalars, tabs → VL0103) · M1b parser + AST + spans + error
recovery · M1c diagnostics rendering (text + `--json`) and `velme check` for syntax only.
**Exit:** all `AC-SYN-*`, `AC-ERR-*` green; golden corpus `tests/golden/parser` covers every EBNF production (accept +
reject).
**User verifies:** `velme check examples/beginner/hello.velme` → ✓ Parsed; a file with a tab shows `VL0103` pointing at it.

### M2 — Semantics

**Read:** `language/11`, `language/12` §call rules, `language/13` §scope + narrowing, `compiler/20` §phases.
**Slices:** M2a name resolution + record types (VL0201–0203, 0205, 0208) · M2b type checking of signatures, calls,
checks, examples incl. narrowing (VL0204, 0206, 0207) · M2c call graph, cycles, binding order, wired goals (VL03xx).
**Exit:** all `AC-TYP-*`, `AC-GOAL-*` (static parts), `AC-CHK-*` (static parts), AC-CMP-03/04 green; AC-RDM-04,
AC-RDM-05 green.
**User verifies:** `velme check` on the cycle and type-mismatch examples prints the friendly errors from `language/12`.

### M3 — IR, validator, interpreter, check evaluator, store + lock

**Read:** `compiler/21` (all), `language/14`, `language/13` §evaluation, `runtime/30` §interpreter semantics,
`runtime/32` (fingerprints, store, manifest, lock, staleness).
**Slices:** M3a IR types + JSON Schema (schemars) + canonical JSON (D-21) · M3b validator stages (schema → budget
analysis) · M3c decimal `Number` + builtins incl. `random` (D-36, D-22) · M3d interpreter + check/example evaluation with
failure reports (VL05xx, VL0602) · M3e fingerprints (D-11) + artifact store + manifest · M3f `velme.lock` read/write,
staleness (D-12), fixture installer in `velme-test-support`. IR for tests is hand-written (no LLM yet) and installed
into the store and lock by that helper (D-16).
**Exit:** all `AC-IR-*`, `AC-BLT-*`, `AC-CHK-*` green; AC-ART-04/06/08/12, AC-CMP-02, AC-SEC-04 green; AC-RDM-06 green
on hand-written IR.
**User verifies:** `velme run` on an example with a hand-written IR fixture shows the result; a broken fixture shows the
failed assertion with expected/received values.

### M4 — VibeVM runtime

**Read:** `runtime/30` (all), `language/12` §DAG and waves, `tooling/40` §explain/trace.
**Slices:** M4a call planner + wave scheduler on Tokio, source-order results and failures (D-9) · M4b budgets: fuel
in the interpreter, calls, depth, list/output size, watchdog (D-10) · M4c trace model, `velme trace`, `velme explain`.
**Exit:** all `AC-RUN-*` green; AC-RDM-02, 03, 07 and 09 (interpreter), AC-QA-05 green; determinism test repeats each
example ×50.
**User verifies:** `velme explain` on `examples/intermediate/player_summary.velme` shows "At the same time: …";
`velme trace` lists calls in source order.

### M5 — Spellbook

**Read:** `compiler/22` (all), `tooling/41` §threat model + secrets, `compiler/21` §validation, `tooling/40` §5.
**Slices:** M5a `SynthProvider` trait, `SynthRequest` + its JSON Schema, `scripted` + `replay` providers, prompt
template v1 · M5b retry loop with diagnostics feedback, verification pipeline, test-input generation,
`VL0403` cause summary and question replies (`VL0407`, R-SYNTH-31..33, D-43), token-cost options (R-SYNTH-34..40, D-44) · M5c Anthropic
provider (config, key from env, structured output, timeouts), recorded replay fixtures for every `examples/` goal ·
M5d `ollama` provider (digest resolution, mock-server tests) and `external` backend (protocol, command sourcing,
env scrubbing, `VL0406`, pending replies `VL0408`) with a small test backend in `velme-test-support` (D-41, D-42, D-45).
**Exit:** all `AC-SYNTH-*` green on scripted/replay; AC-ART-01/02/03/09/10, AC-CMP-05/06/08, AC-SEC-09, AC-QA-02,
AC-REL-03/05 green; one opt-in live run per example recorded as fixtures; AC-RDM-01 and AC-RDM-08 green on replay.
**User verifies:** with an API key, `velme build examples/beginner/add.velme` synthesizes and verifies; without one,
`VL0405` explains how to configure it. Running `velme build` twice makes 0 provider calls the second time; editing one
leaf's plan re-synthesizes only that leaf. With Ollama running, `velme build --provider ollama --model <model> …` does
the same with no key; `velme build --provider external --external-command "<backend>" …` builds from a backend's IR.

### M6 — CLI completion: locked/offline, artifact, config

**Read:** `runtime/32` §locked mode, `tooling/40` (all).
**Slices:** M6a `--locked`, `--offline`, `artifact` command · M6b remaining CLI (input/output mapping D-23, config
file, exit codes, cache commands).
**Exit:** AC-ART-05, AC-ART-07, AC-ART-11, all `AC-CLI-*` green; AC-SEC-08 green.
**User verifies:** `velme run --locked` works with the network off; `velme artifact` shows a goal's IR; a stale goal
under `--locked` exits 4 with `VL0702`.

### M7 — WASM backend + Wasmtime sandbox

**Read:** `runtime/31` (all), `tooling/41` §capabilities, `delivery/51` §differential testing.
**Slices:** M7a IR → core WASM emitter for leaf goals + wasmparser validation · M7b Wasmtime embedding: fuel,
epoch, `ResourceLimiter`, host-function whitelist, no WASI · M7c differential test interpreter vs WASM over all
examples and golden IR; backend selection flag.
**Exit:** all `AC-SBX-*`, AC-SEC-01/07 green; AC-RDM-07, AC-RDM-09 green on WASM; AC-QA-06 (fuzz smoke) green.
**User verifies:** `velme run --backend wasm` gives byte-identical output to `--backend interp` on every example.

### M8 — MVP gate

**Read:** `delivery/50` §success criteria, `delivery/51` §gates, `delivery/52` §release.
**Build:** close the checklist above; examples across beginner/intermediate/games/professional; README quick start;
`CHANGELOG`; release workflow dry run.
**Exit:** every row in the MVP gate checklist ticked with evidence; AC-CMP-07, AC-QA-04, AC-QA-07, AC-REL-04 green;
`cargo xtask verify` green; AC coverage audit clean.
