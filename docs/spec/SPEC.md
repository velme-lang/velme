# Velme — Specification Index

**Velme** is an intent-driven programming language that a child can start with and a professional can choose
deliberately. A program declares typed **goals**; each goal states its intent in a natural-language `plan`, the other
goals it may use in a `call` block, and executable `check` assertions. An LLM (**Spellbook**) synthesizes a typed
**Velme IR** implementation from the plan; a deterministic validator, a reference interpreter and a sandboxed runtime
(**VibeVM**) decide whether that implementation is accepted and run it.

> **Velme is a language and compiler first; the LLM is a synthesis backend, not the language runtime.**

- **Implementation:** Rust workspace — Chumsky parser, serde IR + JSON Schema, reference interpreter, Tokio DAG
  scheduler, Wasmtime sandbox (WASM backend after the interpreter is stable).
- **Surface (v0.1):** `type`, `goal`, `call`, `plan`, `check`, `examples`, optional `budget`.
- **Works offline:** with a committed lockfile, `velme check/run/test` need no network and no account (INV-7).
- **Open core:** language, compiler, runtime, CLI and spec are open source; hosted services are separate products.

---

## 1. How to use this spec

`SPEC.md` is the only file meant to be read every time. Everything else is loaded on demand.
Each child file states what it is for in its first five lines, so you can route without reading the body.

### Routing table

| If you are working on… | Read |
|---|---|
| Anything at all | this file (invariants, principles, decisions index) |
| Lexer, indentation, grammar, literals, file header | `language/10` |
| Types, records, nullability, `Number` semantics, equality, JSON mapping | `language/11` |
| `goal` / `call` / `plan` / `examples` / `budget`, call rules, the call DAG | `language/12` |
| The `check` assertion language | `language/13` |
| Built-in functions, `random`, builtins versioning | `language/14` |
| Crate layout, compiler phases, diagnostics, error reporting | `compiler/20` |
| Velme IR nodes, JSON schema, IR validation | `compiler/21` |
| Spellbook: provider trait, prompt contract, retry, verification pipeline, test inputs | `compiler/22` |
| Interpreter semantics, DAG scheduler, failure semantics, determinism, trace | `runtime/30` |
| WASM backend, Wasmtime sandbox, fuel/epoch/memory limits, host functions | `runtime/31` |
| Fingerprints, artifacts, manifest, lockfile, cache layout, Coach (Future) | `runtime/32` |
| CLI commands, input/output format, explain, trace, exit codes | `tooling/40` |
| Capability model, threat model, secrets, learner privacy | `tooling/41` |
| Sequencing, MVP scope, what is not MVP, success criteria | `delivery/50` |
| Tests, golden files, fuzzing, differential testing, benchmarks, quality gates | `delivery/51` |
| Repo layout, CI, releases, distribution, license, governance, RFCs | `delivery/52` |
| Error codes, failure kinds, reserved words, glossary | `reference/90` |
| Where a section of the original design doc went | `reference/91` |
| Why a design choice was made; open questions for the owner | `reference/92` |

### Identifier scheme

| Prefix | Meaning | Defined in |
|---|---|---|
| `INV-n` | architectural invariant | this file |
| `P-n` | design principle | this file |
| `D-n` | design decision (resolves a gap or contradiction in the original doc) | `reference/92` |
| `Q-n` | open question for the project owner | `reference/92` |
| `R-<AREA>-nn` | binding rule | the owning child file |
| `AC-<AREA>-nn` | testable acceptance criterion | the owning child file |
| `VLnnnn` | stable diagnostic / error code | `reference/90` |
| `CC-*` | code convention | `docs/code-conventions.md` |
| `§n` | section of the original design doc (deleted; the spec replaces it) | `reference/91` |

Areas: `SYN` 10 · `TYP` 11 · `GOAL` 12 · `CHK` 13 · `BLT` 14 · `CMP` 20 · `IR` 21 · `SYNTH` 22 · `RUN` 30 ·
`SBX` 31 · `ART` 32 · `CLI` 40 · `SEC` 41 · `RDM` 50 · `QA` 51 · `REL` 52 · `ERR` 90.

### Status tags

`v0.1` — build now · `v0.1-ready` — reserved (keyword, IR field, schema slot) in v0.1, feature deferred ·
`Future` — do not build, do not contradict.

---

## 2. Architectural invariants

These hold for the life of the language. A change that breaks one is an architecture decision, not a patch.

| ID | Invariant |
|---|---|
| **INV-1** | The LLM never produces executable code. It produces Velme IR, and nothing executes until that IR passes the deterministic validator (`compiler/21`). The validator — not the prompt — is the trust boundary. |
| **INV-2** | `check` assertions are evaluated by the runtime with deterministic semantics; an LLM never decides whether a result passes. |
| **INV-3** | Same source + same inputs + same locked artifacts + same seed ⇒ same result and same trace, on every backend. The reference interpreter defines the semantics; any other backend must match it. |
| **INV-4** | Goals have no ambient authority: no filesystem, network, environment, clock or process access. A goal can use only its inputs, its declared calls and whitelisted built-ins/host functions. |
| **INV-5** | Every execution is bounded: fuel, memory, goal calls, call depth, list size, output size and a wall-clock watchdog. |
| **INV-6** | The call graph is fixed by source before synthesis and is acyclic. A goal calls only goals named in its own `call` block; synthesized IR cannot add, remove or reorder calls (D-5). |
| **INV-7** | The language core works without a network or an account when artifacts are locked. Synthesis providers sit behind one provider-neutral trait; no vendor type or SDK leaks into core crates. |
| **INV-8** | Artifacts are content-addressed and immutable; nothing is cached or looked up by goal name alone. Every artifact records the language, compiler, IR, builtins, prompt and model versions that produced it. |
| **INV-9** | Crate dependencies flow one way: `syntax → sema → ir → (interp, wasm) → runtime → cli` (`diagnostics` and `builtins` are leaf crates any layer may use); core crates never depend on the CLI or on a concrete LLM provider (`compiler/20`). |
| **INV-10** | Diagnostic codes are stable: once released, an `VLnnnn` code is never reused for a different meaning. |

## 3. Design principles

| ID | Principle |
|---|---|
| **P-1** | **Language first** — the spec defines Velme; the implementation and the LLM never silently become the definition. |
| **P-2** | **Progressive disclosure** — one semantic model from the first beginner goal to professional code; advanced features add to it, never replace it. |
| **P-3** | **Reject rather than guess** — prefer a simple rule with a friendly diagnostic over clever inference (e.g. call bindings in source order, §12.5). |
| **P-4** | **Interpreter first** — the reference interpreter stabilizes semantics; WASM is an optimization backend. |
| **P-5** | **Small v0.1** — no recursion, loops, conditionals in calls, generics, effects or modules until the straight-line DAG model is stable. |
| **P-6** | **Friendly words, stable codes** — human messages are written for a learner; codes and JSON output are for tools. |
| **P-7** | **Open core** — nothing in the language, compiler or runtime depends on a hosted service. |

---

## 4. Scope

| Area | v0.1 | v0.1-ready (reserved) | Future |
|---|---|---|---|
| Declarations | `type`, `goal`, `call`, `plan`, `check`, `examples`, `budget` | `pure`, `effects`, `when`, `choose`, `otherwise`, `import`, `module` keywords | conditional calls, `choose`, modules/packages |
| Types | `Number`, `Text`, `Boolean`, `Nothing`, `T?`, `List<T>`, records | — | maps, enums, tuples, generics, ADTs, binary `Float` (D-36) |
| Control | straight-line call DAG; collection primitives and expression `if` inside IR (`compiler/21` R-IR-06) | — | conditional calls, recursion, loops, `fallback`/`retry`/optional calls |
| Synthesis | `anthropic` + local `ollama` providers, `external` backend for human/tool-written IR, structured IR output, retry with diagnostics, replay provider | provider routing fields in manifest | multiple agents, fine-tuning |
| Execution | reference interpreter, parallel DAG scheduler, budgets, trace | — | distributed execution |
| Backend | core-WASM for leaf goals via Wasmtime (last MVP phase) | — | Component Model + WIT |
| Artifacts | local content-addressed store + `velme.lock` | remote store fields | Redis/object storage/PostgreSQL, Coach, hot swap |
| Tooling | CLI: `check build run test explain trace artifact` | — | playground, LSP, Tree-sitter grammar, VS Code |

Full MVP list and exclusions: `delivery/50`.

## 5. Components

| Component | Crate | Owner spec | Status |
|---|---|---|---|
| Syntax (lexer, parser, AST, spans) | `velme-syntax` | `language/10` | v0.1 |
| Diagnostics (codes, rendering) | `velme-diagnostics` | `compiler/20`, `reference/90` | v0.1 |
| Semantic analysis (names, types, call graph) | `velme-sema` | `language/11..13`, `compiler/20` | v0.1 |
| Velme IR + schema + validator | `velme-ir` | `compiler/21` | v0.1 |
| Check DSL lowering + evaluation | `velme-check` | `language/13` | v0.1 |
| Built-ins | `velme-builtins` | `language/14` | v0.1 |
| Reference interpreter | `velme-interp` | `runtime/30` | v0.1 |
| Spellbook (synthesis, providers, verification) | `velme-synth` | `compiler/22` | v0.1 |
| VibeVM (scheduler, budgets, trace, artifact store) | `velme-runtime` | `runtime/30`, `runtime/32` | v0.1 |
| WASM backend + Wasmtime sandbox | `velme-wasm` | `runtime/31` | v0.1 (last MVP phase) |
| CLI | `velme-cli` (binary `velme`) | `tooling/40` | v0.1 |
| Test support (fixtures, golden runner) | `velme-test-support` | `delivery/51` | v0.1 |
| Coach, Playground, Velme Cloud | — | `runtime/32` §Future, `delivery/50` | Future |

## 6. Architecture decisions (summary)

| Decision | Choice | Ref |
|---|---|---|
| LLM output boundary | typed Velme IR (JSON), never WASM | INV-1, `compiler/21` |
| Implementation language | Rust (stable toolchain pinned) | `compiler/20` |
| Parser | Chumsky + hand-written indentation-aware lexer; Tree-sitter later, sharing the golden corpus | D-25 |
| Semantics owner | reference interpreter; WASM must match (differential tests) | P-4, `delivery/51` |
| `Number` | exact decimal (28 fractional digits), not configurable | D-36 |
| Randomness | explicit seed input only; SplitMix64-based `random(seed, index)` | D-22 |
| Composite goals | call DAG compiled from source; LLM synthesizes only the final combination | D-5 |
| Cache key for synthesis | child *signatures*, not child implementations | D-11 |
| Reproducibility | `velme.lock` + `.velme/artifacts/` content-addressed store | D-12 |
| Hashing | BLAKE3 over canonical JSON | D-21 |
| Tests without an LLM | `replay` + `scripted` providers; live LLM tests opt-in only | D-13 |
| MVP providers | Anthropic and Ollama (structured output, model configurable); `external` protocol for human/tool-written IR | D-14, D-41, D-42 |
| Runtime | Tokio scheduler; Wasmtime fuel + epoch + `ResourceLimiter` | `runtime/30..31` |
| Error codes | `TL` + 4 digits, grouped by phase | D-17 |
| License | MIT OR Apache-2.0 code, CC BY 4.0 spec/docs, trademark kept separate | D-38, `delivery/52` |

## 7. Source of truth

| Artifact | Owner |
|---|---|
| Language semantics | this spec tree (later mirrored as the versioned `language/spec/0.1/`, `delivery/52`) |
| Accepted implementation of a goal | the locked IR artifact (`runtime/32`) |
| Whether a result passes | the runtime's check evaluator (INV-2) |
| Diagnostic codes | `reference/90` |

---

## 8. File tree

```
docs/spec/
  SPEC.md                         this file
  language/
    10-syntax-grammar.md          lexing, indentation, EBNF, literals, header, keywords
    11-types.md                   type system, records, nullability, Number, equality, JSON mapping
    12-goals-calls.md             goal/plan/call/examples/budget, call rules, DAG and waves
    13-check-dsl.md               assertion language: grammar, scope, narrowing, lowering
    14-builtins.md                built-in catalog, random, builtins version
  compiler/
    20-compiler-architecture.md   crates, phases, diagnostics pipeline
    21-ir.md                      IR nodes, JSON schema, validation rules, IR versioning
    22-spellbook-synthesis.md     provider trait, prompt contract, retry, verification, test inputs
  runtime/
    30-execution-vibevm.md        interpreter, DAG scheduler, failures, determinism, trace, budgets
    31-wasm-sandbox.md            WASM backend, Wasmtime limits, host functions, Component Model path
    32-artifacts-cache.md         fingerprints, manifest, lockfile, cache layout, Coach (Future)
  tooling/
    40-cli.md                     commands, I/O formats, explain, trace, exit codes
    41-security-privacy.md        capabilities, threat model, secrets, learner privacy
  delivery/
    50-roadmap.md                 phases, MVP scope, exclusions, success criteria
    51-testing-quality.md         test layers, golden/fuzz/differential, benches, gates
    52-repo-release-governance.md repo layout, CI, releases, distribution, license, RFCs
  reference/
    90-errors-glossary.md         TL codes, failure kinds, reserved words, glossary
    91-coverage-map.md            original § → owning file
    92-decisions-questions.md     design review: decisions D-n, open questions Q-n
```
