# 91 — Coverage Map

**Status:** Reference · **Area:** —
**Read when:** checking that a section of the original design doc survived the split, or auditing spec completeness.
**Depends on:** [SPEC](../SPEC.md), [92-decisions-questions](92-decisions-questions.md)
**Source:** the whole original design doc (3,659 lines; deleted after the split — this map is the record)

## Purpose

The original design doc (written under the name *Neya*; renamed via *Nela* to *Thela* per D-1) was split into the `docs/spec/`
tree. It is **superseded**: do not read it for implementation — where it and the spec differ, the spec wins, usually
because of a decision in `reference/92`. This table maps every section to the file that now owns its substance.
Content in several places is summarised in the owner and cross-referenced elsewhere.

## Map

| § | Original section | Owner file | Notes |
|---|---|---|---|
| §1 | Executive Summary | `SPEC.md` | pipeline → `compiler/20`; LLM boundary → INV-1 |
| §1A | Product Positioning: Kids First, Professional by Design | `SPEC.md` | P-2; examples' `pure`/`budget`/untyped params → D-3, D-8, F-4..F-6 |
| §1A | — For kids / intermediate / professional | `SPEC.md` | untyped param Future (D-3); `FindMaximum(by=…)` Future (F-6) |
| §1A | — Progressive disclosure | `SPEC.md` | P-2 |
| §1A | — Friendly surface, serious foundation | `SPEC.md` | P-2, P-6 |
| §2 | Design Goals | `SPEC.md` | |
| §2.1 | User goals | `SPEC.md` | |
| §2.2 | Engineering goals | `SPEC.md` | invariants INV-1..10 |
| §3 | Important Corrections to the Existing Specification | `SPEC.md` | each correction owned below |
| §3.1 | LLM-generated WASM → LLM-generated Thela IR | `compiler/21` | INV-1 |
| §3.2 | `check` must be deterministic | `language/13` | INV-2 |
| §3.3 | Pure functions conflict with uncontrolled randomness | `language/14` | D-22 |
| §3.4 | 50 ms limit as part of a resource budget | `runtime/30` | INV-5, D-10; `max_list_size` restored (F-21) |
| §4 | Thela v0.1 Language Model | `language/12` | + `examples`, `budget` (D-7, D-8) |
| §5 | Types | `language/11` | |
| §5.1 | Primitive types | `language/11` | `Number` semantics D-36 (D-2 retired) |
| §5.2 | Named record types | `language/11` | |
| §5.3 | v0.1 type rules | `language/11` | |
| §6 | Goal Syntax | `language/12` | grammar in `language/10`; example fixed (F-3) |
| §7 | `check` Syntax | `language/13` | additions D-6 |
| §7.1 | Comparisons | `language/13` | |
| §7.2 | Boolean logic | `language/13` | + `if … then` |
| §7.3 | Field access | `language/13` | narrowing D-6 |
| §7.4 | Collection helpers | `language/14` | used from `language/13` |
| §7.5 | Empty checks | `language/13` | |
| §7.6 | Universal assertions | `language/13` | + `some` |
| §8 | Multi-Function Calls | `language/12` | |
| §8.1 | Why a separate `call` block? | `language/12` | INV-6 |
| §9 | Multi-Function Calls: Parallel Case | `language/12` | waves; execution in `runtime/30` |
| §10 | Multi-Function Calls: Sequential Case | `language/12` | |
| §11 | Multi-Function Calls: Mixed Parallel + Sequential | `language/12` | |
| §12 | Call Rules | `language/12` | |
| §12.1 | Calls must be declared | `language/12` | TL0303 |
| §12.2 | Calls must have compatible types | `language/12` | TL0204 |
| §12.3 | A goal cannot call itself in v0.1 | `language/12` | TL0304 |
| §12.4 | Mutual recursion is also rejected | `language/12` | TL0304 |
| §12.5 | Only earlier bindings can be referenced | `language/12` | TL0305, P-3 |
| §13 | Conditional Calls | `delivery/50` | Future; `when`/`choose` reserved (D-24) |
| §14 | Loops and Large Lists | `language/14` | collection primitives; IR nodes in `compiler/21` |
| §15 | Thela Intermediate Representation | `compiler/21` | example corrected (F-1) |
| §16 | IR Node Types | `compiler/21` | `Call` compiler-only (D-5) |
| §17 | Two Kinds of Thela Goals | `language/12` | + wired goals (D-4) |
| §17.1 | Leaf goal | `language/12` | |
| §17.2 | Composite goal | `language/12` | tail synthesis D-5 |
| §18 | Compiler Architecture | `compiler/20` | |
| §19 | Parser Technology | `compiler/20` | Chumsky; Tree-sitter later D-25 |
| §20 | Spellbook / LLM Architecture | `compiler/22` | |
| §20.1 | LLM request | `compiler/22` | prompt contract |
| §20.2 | Structured output | `compiler/22` | provider-neutral validator INV-1 |
| §21 | LLM Retry Strategy | `compiler/22` | max 3 retries (`tooling/40` config) |
| §22 | Verification Pipeline | `compiler/22` | |
| §23 | Testing the Generated Program | `compiler/22` | examples syntax D-7; implementation testing in `delivery/51` |
| §24 | Execution Architecture | `runtime/30` | |
| §24.1 | VibeVM | `runtime/30` | component names `reference/90` §5 |
| §24.2 | GoalRegistry | `runtime/32` | |
| §25 | Artifact Identity and Cache Keys | `runtime/32` | signature fingerprints D-11 |
| §26 | Cache Layout | `runtime/32` | local store + lock D-12; production stores Future |
| §27 | WASM Runtime | `runtime/31` | capabilities also `tooling/41` |
| §28 | CPU and Memory Limits | `runtime/31` | call limits in `runtime/30` |
| §29 | Composite Goal Runtime | `runtime/30` | |
| §30 | Failure Semantics | `runtime/30` | deterministic first failure D-9; kinds in `reference/90` §3 |
| §31 | Determinism | `runtime/30` | INV-3 |
| §32 | `random` Design | `language/14` | runtime seed derivation Future (D-22, F-10) |
| §33 | Debugging Model | `runtime/30` | trace; rendering in `tooling/40` |
| §34 | Explain Mode | `tooling/40` | built from the DAG |
| §35 | Coach / Optimizer | `runtime/32` | Future section; roadmap `delivery/50` §6 |
| §36 | Optimization Safety Rule | `runtime/32` | Future section |
| §37 | MVP Scope | `delivery/50` | §3 |
| §38 | Explicitly NOT MVP | `delivery/50` | §4 |
| §39 | MVP Backend Recommendation | `runtime/31` | P-4; sequencing `delivery/50` |
| §40 | WASM Backend Strategy | `runtime/31` | |
| §41 | Core WASM vs Component Model | `runtime/31` | Component Model Future |
| §42 | Recommended Technology Stack | `compiler/20` | runtime stack `runtime/30..31`; cache `runtime/32` |
| §42A | Open Source Strategy and Project Governance | `delivery/52` | P-7 |
| §42A.1 | Why the language core should be open source | `delivery/52` | |
| §42A.2 | Recommended license | `delivery/52` | D-38 |
| §42A.3 | Keep Thela useful without Thela Cloud | `delivery/52` | INV-7; commands in `tooling/40` |
| §42A.4 | Commercial boundary | `delivery/52` | |
| §42A.5 | Kids-to-professional ecosystem | `SPEC.md` | P-2 |
| §43 | Source Hosting and Repository Structure | `delivery/52` | |
| §43.1 | Best source-code host: GitHub | `delivery/52` | org `thela-lang` |
| §43.2 | One public core monorepo + separate product repositories | `delivery/52` | §8 |
| §43.3 | Recommended public monorepo layout | `delivery/52` | right-sized per D-15 (F-16) |
| §43.4 | Why this structure works | `delivery/52` | golden/fuzz/bench in `delivery/51` |
| §43.5 | Dependency direction | `delivery/52` | INV-9; crate edges `compiler/20` |
| §43.6 | Public repository ownership rules | `delivery/52` | |
| §43.7 | CI/CD | `delivery/52` | gate steps `delivery/51` §4 |
| §43.8 | Release channels | `delivery/52` | |
| §43.9 | Distribution | `delivery/52` | |
| §43.10 | Documentation hosting | `delivery/52` | |
| §43.11 | Security baseline | `tooling/41` | |
| §43.12 | Recommended initial repository set | `delivery/52` | |
| §43.13 | GitHub project setup | `delivery/52` | |
| §43.14 | Recommended first commit | `delivery/52` | |
| §43.15 | Naming and repository availability | `delivery/52` | redone for Thela 2026-09-25 (D-38, F-24) |
| — | Repository Structure — Summary | — | pointer only; no content |
| §44 | Example: Complete Thela Program | `language/12` | also `examples/intermediate` |
| §45 | Example: Parallel Game Preparation | `language/12` | also `examples/games` |
| §46 | Example: Multi-Step Game Physics | `language/12` | also `examples/games` |
| §47 | CLI | `tooling/40` | output corrected (F-2); inputs D-23; `test` added (F-21) |
| §48 | Language Versioning | `language/10` | header; version set in `delivery/52` §5 |
| §49 | Security Architecture | `tooling/41` | INV-4 |
| §50 | Error Taxonomy | `reference/90` | renumbered TLnnnn (D-17) |
| §51 | MVP Roadmap | `delivery/50` | reordered M0–M8 (D-16, F-15) |
| §52 | Success Criteria for MVP | `delivery/50` | AC-RDM-01..09; Test 2 fixed (F-7) |
| §53 | Recommended First Prototype | `delivery/50` | phases M0–M6 |
| §54 | Final Recommended Architecture | `SPEC.md` | §5 components; pipeline `compiler/20` |
| §55 | Bottom Line | `SPEC.md` | headline quote |
| §56 | Research / Technology References | `compiler/20` | tool choices only; citations dropped |
