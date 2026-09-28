# 20 — Compiler Architecture

**Status:** v0.1 · **Area:** CMP
**Read when:** creating or wiring a crate, deciding where code belongs, adding a compiler phase, or changing how diagnostics are produced or rendered.
**Depends on:** [SPEC](../SPEC.md), [21-ir](21-ir.md), [22-spellbook-synthesis](22-spellbook-synthesis.md), [90-errors-glossary](../reference/90-errors-glossary.md)
**Source:** §18, §19, §42, §43.4, §43.5, §53, §54

## 1. Purpose & boundaries

Defines the Rust workspace, the compiler phases and what each consumes and produces, the diagnostics pipeline, and the
public compiler API that the CLI, a future LSP and the playground share. Syntax is owned by
[language/10](../language/10-syntax-grammar.md); type rules by [language/11](../language/11-types.md); IR shape by
[21-ir](21-ir.md); execution by [runtime/30](../runtime/30-execution-vibevm.md).

## 2. Workspace crates (D-15)

| Crate | Responsibility | May depend on (workspace) |
|---|---|---|
| `velme-diagnostics` | `Diagnostic` type, `VLnnnn` code enum, labels/spans, ariadne + JSON renderers | — |
| `velme-syntax` | indentation-aware lexer, Chumsky parser, AST with spans, literal parsing | `diagnostics` |
| `velme-builtins` | built-in catalog: names, signatures, pure Rust implementations, `builtins_version` | `diagnostics` |
| `velme-sema` | name resolution, type checking, narrowing, call-graph build + cycle check → typed semantic model (HIR) | `syntax`, `builtins`, `diagnostics` |
| `velme-ir` | IR types, JSON Schema (schemars), canonical JSON, validator, lowering HIR → IR for compiler-owned parts | `sema`, `builtins`, `diagnostics` |
| `velme-check` | lowering of `check`/`examples` to IR predicates; check evaluation on values | `ir`, `sema`, `diagnostics` |
| `velme-interp` | reference interpreter for IR (fuel-metered, pure) | `ir`, `builtins`, `diagnostics` |
| `velme-synth` | Spellbook: `SynthProvider` trait, providers, prompt builder, retry loop, verification pipeline, test-input generator | `ir`, `check`, `interp`, `sema`, `diagnostics` |
| `velme-wasm` | IR → core WASM emitter, Wasmtime sandbox host (M7) | `ir`, `builtins`, `diagnostics` |
| `velme-runtime` | VibeVM: goal registry, call planner, scheduler, budgets, trace, artifact store, lockfile | `ir`, `check`, `interp`, `wasm`, `synth`, `diagnostics` |
| `velme-cli` | binary `velme`: argument parsing, config, rendering, exit codes | any of the above |
| `velme-test-support` | golden runner, fixture loaders, scripted provider helpers (dev-dependency only) | any library crate |

**R-CMP-01** Crate dependencies follow the table and INV-9. A dependency not listed there is an architecture change
(stop and raise it). Enforced by a `cargo xtask deps` check (or `cargo-deny` bans) in the gate.
**R-CMP-02** No library crate depends on `velme-cli`, `clap`, a terminal library or an LLM vendor SDK. Vendor HTTP code
lives only inside the provider module of `velme-synth` behind the `SynthProvider` trait (INV-7).
**R-CMP-03** Only `velme-cli` prints, reads environment variables or chooses exit codes. Libraries return values and
`Diagnostic`s.
**R-CMP-04** A new crate needs a real API or ownership boundary (§43.4); a module inside an existing crate is the default.

```
            diagnostics
                 ▲
   ┌─────────────┼───────────────┐
 syntax      builtins            │
   ▲             ▲               │
   └──── sema ───┘               │
          ▲                      │
          ir ◄───────────────────┘
      ▲   ▲    ▲
  check  interp  wasm
     ▲    ▲  ▲     ▲
     synth─┘  │     │
       ▲      │     │
       └── runtime ─┘
              ▲
             cli
```

## 3. Phases

| # | Phase | Input | Output | Crate | Can call an LLM? |
|---|---|---|---|---|---|
| 1 | Lex | source text | tokens incl. `INDENT`/`DEDENT`, block scalars | `syntax` | no |
| 2 | Parse | tokens | AST, every node with a `Span` (file id, byte range) | `syntax` | no |
| 3 | Resolve | AST | symbol tables (types, goals, bindings), version header resolved | `sema` | no |
| 4 | Type-check | resolved AST | typed HIR: goal signatures, typed call bindings, typed checks/examples, budgets | `sema` | no |
| 5 | Call graph | HIR | goal DAG (acyclic, VL0304), per-goal binding DAG and waves | `sema` | no |
| 6 | Classify | HIR + graph | per goal: `leaf` / `composite` / `wired` (D-4) | `sema` | no |
| 7 | Synthesis request | HIR of one goal + child signatures | `SynthRequest` (22 §3) | `synth` | — |
| 8 | Synthesize | `SynthRequest` | candidate IR JSON (cache/lock first) | `synth` | **yes** (only phase) |
| 9 | Validate | candidate IR | validated IR or `VL0401`/`VL0402` diagnostics | `ir` | no |
| 10 | Verify | validated IR + checks + examples | accepted artifact or `VL0503` | `synth` + `interp` + `check` | no |
| 11 | Backend | accepted IR | interpreter plan or WASM module (M7) | `interp` / `wasm` | no |

**R-CMP-05** Phases 1–6 are pure functions of source text (+ builtins version). They never touch the network, the
artifact store or the clock. `velme check` runs phases 1–6 only and therefore never calls an LLM.
**R-CMP-06** The typed HIR is the only thing Spellbook consumes. Synthesis never re-parses source or reads the AST; the
prompt is built from HIR types, signatures, plan text, checks and examples (22 §3).
**R-CMP-07** For `wired` goals (D-4) phases 7–8 are skipped; phase 9 validates the compiler-produced IR like any other.
**R-CMP-08** For `composite` goals the compiler emits the call section (`Call` nodes, compiler-only per D-5) and asks
Spellbook only for the tail; the two are joined before phase 9.
**R-CMP-09** Every phase is keyed per goal. A change to one goal invalidates only that goal's HIR-derived fingerprint
and the synthesis keys that include its **signature** (D-11): editing a leaf's plan does not re-synthesize its parents.
**R-CMP-10** Phases 1–5 recover and continue after errors so one run reports as many independent diagnostics as
possible (Chumsky recovery, poisoned types that suppress cascades). Phases 7+ run only if 1–6 produced no errors.

## 4. Typed semantic model (HIR)

The HIR is the stable contract between front end and back ends:

| Item | Contents |
|---|---|
| `Program` | language version, types, goals (in source order), file id |
| `RecordType` | name, ordered fields `(name, Type)`, span |
| `Type` | `Number`, `Text`, `Boolean`, `Nothing`, `Optional(T)`, `List(T)`, `Record(TypeId)` |
| `Goal` | name, params, output type, kind (`leaf`/`composite`/`wired`), `plan` (normalized, D-21), `call` bindings, `checks`, `examples`, `budget`, spans |
| `Binding` | name, callee `GoalId`, typed args (references to inputs or earlier bindings), result type, wave number |
| `Check` / `Example` | typed expression tree over inputs, bindings and `result` (language/13) |

**R-CMP-11** HIR carries resolved ids, not names; spans survive into HIR so later phases (verification, runtime check
failures) can point back at source.
**R-CMP-12** HIR is `serde`-serializable for golden tests (`tests/golden/sema/*.hir.json`) and for the `velme build
--emit hir` debug view.

## 5. Diagnostics (P-6, INV-10)

**R-CMP-13** Every user-facing error is a `Diagnostic { code: VLnnnn, severity, message, labels: [(Span, text)],
notes, help }`. The code enum lives in `velme-diagnostics` only; codes are never retyped as strings elsewhere (CC-*).
**R-CMP-14** Messages are written for a learner: say what went wrong in plain words, point at the source, and suggest
the fix (`help:`). Jargon (DAG, IR, fuel) appears only in notes or in `--verbose`.
**R-CMP-15** Two renderers: `human` (ariadne, colour when a TTY) and `json` (one object per diagnostic: `code`,
`severity`, `message`, `file`, `span`, `labels`, `notes`, `help`, laid out as in `tooling/40` R-CLI-08, D-68). The
JSON shape is versioned and additive-only; tools and the playground depend on it.
**R-CMP-16** Diagnostics are emitted sorted by (file, start offset, code) so output is deterministic (INV-3).
**R-CMP-17** Runtime failures (`VL05xx`, `VL06xx`) reuse the same `Diagnostic` type, with the check or call span as the
primary label and the offending values as notes (runtime/30 §6).

Example (human renderer):

```
Error: This call gives CalculateScore a Text, but it needs a Player.  [VL0204]
    ╭─[ player.velme:14:32 ]
    │
 14 │         score = CalculateScore("hello")
    │                                ───┬───
    │                                   ╰───── here
    │
    │ Help: pass the `player` input instead
────╯
```

The first line ends with the code (AC-ERR-03, D-72); past 20 diagnostics the rest are counted as "…and N more"
(`language/10` R-SYN-17).

## 6. Public compiler API

The API is the same for CLI, LSP and playground (§43.5). Sketch (names normative, signatures indicative):

```rust
pub struct SourceFile { pub id: FileId, pub path: Utf8PathBuf, pub text: String }

pub fn parse(file: &SourceFile) -> (Option<ast::Program>, Vec<Diagnostic>);            // velme-syntax
pub fn analyze(file: &SourceFile) -> (Option<hir::Program>, Vec<Diagnostic>);          // velme-sema (phases 1–6)
pub fn lower_goal(p: &hir::Program, g: GoalId) -> ir::GoalSkeleton;                     // velme-ir (call section / wired body)
pub fn validate(p: &hir::Program, g: GoalId, ir: &ir::Goal) -> Result<ValidIr, Vec<Diagnostic>>; // velme-ir
pub async fn build(p: &hir::Program, opts: BuildOptions, store: &dyn ArtifactStore,
                   provider: Option<&dyn SynthProvider>) -> BuildReport;                // velme-runtime (D-54)
pub async fn run(p: &hir::Program, lock: &Lockfile, goal: GoalId, input: Value,
                 opts: RunOptions) -> RunOutcome;                                       // velme-runtime
pub fn explain(p: &hir::Program, g: GoalId) -> Explanation;                             // velme-runtime (no LLM)
```

**R-CMP-18** `analyze` is synchronous, allocation-bounded and fast enough for keystroke-level use by an editor
(target: < 50 ms for a 1 000-line file, `delivery/51`).
**R-CMP-19** Everything that can reach the network takes the provider as an explicit argument; passing `None` (the
`--locked`/offline path) makes synthesis impossible by construction (INV-7).
**R-CMP-20** `build` lives in `velme-runtime`, which owns lock/store staleness and the `compiler/22` R-SYNTH-02 lookup
order, calling `velme-synth` only for goals that need synthesis (D-54). `velme-synth` exposes a `ChildRunner` trait
that `velme-runtime` implements and passes into synthesis, so verification (`compiler/22` R-SYNTH-14) can run a
composite goal's real, already-accepted children without `velme-synth` depending on the artifact store.

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-CMP-01 | The dependency check fails the gate if any crate adds a dependency outside §2 (e.g. `velme-ir` → `velme-cli`). |
| AC-CMP-02 | `velme check` on any golden program performs zero network calls and never invokes a provider (asserted with a provider that panics on use); it reads the store only to validate locked IR (`tooling/40` §2). |
| AC-CMP-03 | A file with three independent errors (syntax in one goal, unknown type in another, cycle between two others) reports all three in one run, sorted by position. |
| AC-CMP-04 | `--json` diagnostics (`tooling/40` R-CLI-08) for every golden error file match their snapshots. |
| AC-CMP-05 | Editing only a leaf goal's `plan` changes that goal's synthesis key and no ancestor's (D-11). |
| AC-CMP-06 | A `wired` goal (D-4) builds and runs with no provider configured. |
| AC-CMP-07 | `analyze` on a generated 1 000-line program completes in < 50 ms on the reference machine (bench). |
| AC-CMP-08 | `velme-synth`'s public API takes no `&dyn ArtifactStore`; a `cargo xtask deps` check confirms no new dependency edge outside §2 (D-54). |
