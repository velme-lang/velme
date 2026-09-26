# 90 — Error Codes, Enumerations & Glossary

**Status:** Reference · **Area:** ERR
**Read when:** emitting or matching a diagnostic, adding a code, needing the exact values of a failure kind or reserved
word, or checking what a term means.
**Depends on:** [SPEC](../SPEC.md), [92-decisions-questions](92-decisions-questions.md)
**Source:** §30, §50, §24, glossary terms throughout

## 1. Purpose & boundaries

The single catalog of diagnostic codes (D-17), runtime failure kinds, reserved words and project vocabulary. Rules that
*raise* a code live in the owning spec; this file owns the code, its name and its learner-facing message.

**R-ERR-01** Codes are `TL` + 4 digits, grouped by phase (D-17). A released code is never reused or renamed to a
different meaning (INV-10); a retired code stays in the table marked `retired`.
**R-ERR-02** Each code is defined once in `thela-diagnostics` (name, severity, message template, help template) and
referenced by name everywhere, tests included (CC-CONST).
**R-ERR-03** Messages follow the style guide in `tooling/40` §3.5. `{…}` placeholders below are filled from the
diagnostic's data.

## 2. Diagnostic codes

| Code | Name | Raised in | Meaning | Learner message (sample) | Was |
|---|---|---|---|---|---|
| TL0101 | UnexpectedToken | syntax | parser met a token it can't use here | I didn't expect `{found}` here — I was looking for {expected}. | A001 |
| TL0102 | InconsistentIndentation | syntax | dedent to a width no enclosing block uses | This line's indentation doesn't line up with the lines above it. | A001 |
| TL0103 | TabIndentation | syntax | tab in leading whitespace (D-19) | Please indent with spaces, not tabs. | A001 |
| TL0104 | ReservedWord | syntax | reserved future word used (D-24) | `{word}` is saved for a later Thela version — try another name. | — |
| TL0105 | UnterminatedText | syntax | text literal missing closing quote | This text starts with `"` but never ends. | A001 |
| TL0106 | UnsupportedLanguageVersion | syntax | header names a version this compiler doesn't support | This file is written for `{version}`, but this Thela understands `{supported}`. | — |
| TL0201 | UnknownType | names/types | type name not declared | I don't know a type called `{name}`. | A003 |
| TL0202 | UnknownName | names/types | identifier not in scope | I don't know what `{name}` is here. | — |
| TL0203 | DuplicateDeclaration | names/types | type/goal/field/parameter declared twice | `{name}` is already defined on line {line}. | — |
| TL0204 | TypeMismatch | names/types | value's type differs from the expected type | Expected {expected}, but got {found}. | A004 |
| TL0205 | UnknownField | names/types | record has no such field | A `{type}` doesn't have a field called `{field}`. | — |
| TL0206 | InvalidOperandType | names/types | operator applied to unsupported types | `{op}` can't be used with {left} and {right}. | A004 |
| TL0207 | NullableAccess | names/types | field access on un-narrowed `T?` (D-6) | `{expr}` might be empty — check `is not empty` first. | — |
| TL0208 | RecursiveType | names/types | record type contains itself directly or indirectly | `{type}` contains itself, which Thela doesn't allow yet. | — |
| TL0301 | UnknownGoal | calls/graph | call names an undeclared goal | I don't know a goal called `{name}`. | A002 |
| TL0302 | CallArityMismatch | calls/graph | wrong number of arguments | `{goal}` needs {expected} inputs, but got {found}. | A006 |
| TL0303 | InvalidCall | calls/graph | call not allowed here (e.g. goal used outside its `call` block, self-call outside `examples`) | `{goal}` can only be used after it's listed in `call:`. | A006 |
| TL0304 | CallCycle | calls/graph | cycle in the goal call graph, incl. self-call | These goals call each other in a circle: {cycle}. | A005 |
| TL0305 | BindingUsedBeforeDefinition | calls/graph | binding referenced before its line (§12.5) | `{name}` is used before it's made — move its line up. | — |
| TL0306 | DuplicateBinding | calls/graph | two bindings share a name, or a binding shadows an input | `{name}` is already used in this goal. | — |
| TL0307 | GoalHasNoBody | calls/graph | goal has neither `plan` nor `result` binding (D-4) | `{goal}` needs a `plan:` that says what it should do. | — |
| TL0308 | InvalidBudget | calls/graph | `budget` line has an unknown key or unit, a bad or repeated value, or a value above the system cap (D-8, `language/12` R-GOAL-20) | `budget` can only make limits smaller — `{key}` can be at most `{cap}`. | — |
| TL0401 | IRSchemaInvalid | IR/synthesis | IR JSON fails the schema | The generated program wasn't in the right shape. | A007 |
| TL0402 | IRInvalid | IR/synthesis | IR schema-valid but fails validation (names, types, capabilities, `Call`) | The generated program broke a rule: {rule}. | A007 |
| TL0403 | SynthesisFailed | IR/synthesis | no accepted IR after max retries, or `max_calls_per_build` reached; states the cause (`compiler/22` R-SYNTH-31) | Thela couldn't build `{goal}`: {cause} ({count} of {tries} tries). | A008 |
| TL0404 | ProviderUnavailable | IR/synthesis | provider unreachable, rate-limited, or `--offline` | Thela couldn't reach the AI helper to build `{goal}`. | — |
| TL0405 | ProviderNotConfigured | IR/synthesis | no provider/model/API key configured, or the Ollama server lacks the model | To build `{goal}`, set `{env_var}` to your API key. | — |
| TL0406 | BackendFailed | IR/synthesis | `external` backend exited non-zero, timed out, replied with non-JSON or oversized output, or returned `{"error"}` (D-42) | The backend `{backend}` couldn't build `{goal}`: {reason} | — |
| TL0407 | PlanUnclear | IR/synthesis | the AI helper or `external` backend replied with a question instead of IR (`compiler/22` R-SYNTH-32, D-43) | Thela needs more detail to build `{goal}`. Add the answer as an example, or to the plan, and build again. | — |
| TL0501 | CheckFailed | checks | a `check` assertion evaluated false | `{goal}` didn't pass its check: `{check}`. | A009 |
| TL0502 | ExampleFailed | checks | an `examples:` item produced a different value | For {given}, `{goal}` gave {got} but the example expects {expected}. | A009 |
| TL0503 | VerificationFailed | checks | candidate IR failed checks/examples during build (per-attempt; final is TL0403) | The generated program didn't pass `{check}` for {input}. | A009 |
| TL0601 | BudgetExceeded | runtime | fuel exhausted (deterministic, D-10) | `{goal}` took too many steps and was stopped. | A010 |
| TL0602 | ArithmeticError | runtime | divide by zero, overflow, non-integer or out-of-range integer argument (D-22, D-36) | `{goal}` tried to {op}, which has no answer. | — |
| TL0603 | Timeout | runtime | wall-clock watchdog fired (non-reproducible, D-10) | `{goal}` ran too long and was stopped. | A011 |
| TL0604 | MemoryLimitExceeded | runtime | memory limit reached | `{goal}` needed more memory than it's allowed. | A012 |
| TL0605 | CallLimitExceeded | runtime | `max_goal_calls` or `max_call_depth` exceeded | Too many goals were called while running `{goal}`. | A010 |
| TL0606 | SizeLimitExceeded | runtime | list size or output size cap exceeded | `{goal}` made a list or answer that's too big. | A010 |
| TL0607 | InternalError | any | invariant violated inside Thela (a bug) | Something went wrong inside Thela. Please report it: {report_url}. | — |
| TL0701 | ArtifactUnavailable | artifacts | referenced artifact missing from store | The built version of `{goal}` is missing — run `thela build`. | A013 |
| TL0702 | LockStale | artifacts | lock entry missing or fingerprint mismatch under `--locked` | `{goal}` changed since it was last built — run `thela build`. | — |
| TL0703 | ArtifactCorrupt | artifacts | artifact bytes don't match their hash | The built version of `{goal}` was changed or damaged. | — |
| TL0801 | CapabilityDenied | capabilities | IR or module requests an ungranted capability/host function | `{goal}` tried to use `{capability}`, which goals aren't allowed to use. | A014 |
| TL0901 | FileError | CLI/IO | source/config/input file unreadable | I couldn't open `{path}`. | — |
| TL0902 | InvalidInput | CLI/IO | input JSON or config doesn't match expected shape (D-23) | Input `{name}` should be {expected}, but got {found}. | — |
| TL0903 | GoalNotFound | CLI/IO | `--goal` names no goal in the file | There's no goal called `{name}`. Did you mean `{suggestion}`? | — |

Severity: all codes above are errors in v0.1. Warnings, when added, use the same groups with names ending in `Warning`.

## 3. Runtime result and failure kinds

Each goal invocation ends in exactly one outcome (§30, `runtime/30`):

| Kind | Code | Reproducible |
|---|---|---|
| `Success(value)` | — | yes |
| `CheckFailed` | TL0501 | yes |
| `BudgetExceeded` | TL0601 | yes |
| `ArithmeticError` | TL0602 | yes |
| `CallLimitExceeded` | TL0605 | yes |
| `SizeLimitExceeded` | TL0606 | yes |
| `MemoryLimitExceeded` | TL0604 | yes (limit is deterministic on each backend) |
| `Timeout` | TL0603 | **no** (D-10) |
| `CapabilityDenied` | TL0801 | yes |
| `Unavailable` | TL0701 / TL0404 | no (environment) |
| `ChildFailed(binding, kind)` | child's code | as child (D-9) |

Original `Failure(error)` and `ValidationFailed` map to the specific kinds above.

## 4. Reserved words

| Word | Status |
|---|---|
| `language type goal call plan check examples budget result nothing true false and or not is empty every some in has if then` | v0.1 keywords |
| `pure effects when choose otherwise import module fallback retry optional assume` | reserved (D-24) → `TL0104` |

Exact lexical rules (which words are contextual) are owned by `language/10`.

## 5. Component names

| Name | What it is | Crate |
|---|---|---|
| **Spellbook** | synthesis subsystem: prompt, provider, retry, verification | `thela-synth` |
| **VibeVM** | runtime: scheduler, budgets, trace, artifact store, backends | `thela-runtime` (+ `thela-interp`, `thela-wasm`) |
| **Coach** | background optimizer (Future) | — |
| **Thela Cloud** | hosted product (Future, private) | — |

## 6. Glossary

| Term | Meaning |
|---|---|
| **goal** | a typed, named unit of behaviour: signature + optional `call`, `plan`, `check`, `examples`, `budget` |
| **leaf goal** | goal with no `call` block; its whole body is synthesized IR |
| **composite goal** | goal with a `call` block; the DAG is compiled, the tail is synthesized (D-5) |
| **wired goal** | composite goal whose `call` block binds `result`; no synthesis (D-4) |
| **tail** | the synthesized IR expression computing a composite goal's output from inputs and bindings |
| **binding** | a name bound to a call's result in a `call` block |
| **call DAG** | dependency graph of a goal's bindings; acyclic by construction (INV-6) |
| **wave** | set of bindings whose dependencies are all complete; run concurrently |
| **plan** | natural-language intent; input to synthesis, documentation for wired goals |
| **check** | deterministic assertion over inputs, bindings and `result` (INV-2) |
| **example** | an equality between a call of the goal on literals and an expected value (D-7) |
| **narrowing** | refining `T?` to `T` after `is not empty` in a check (D-6) |
| **IR** | Thela intermediate representation; typed JSON the LLM emits and the validator accepts (INV-1) |
| **artifact** | immutable, content-addressed accepted IR plus manifest (INV-8) |
| **manifest** | artifact metadata: versions, fingerprints, provider/model, verification record |
| **fingerprint** | BLAKE3 hash over canonical inputs identifying a synthesis request or artifact (D-11, D-21) |
| **signature fingerprint** | hash of a goal's name, parameter and output types only |
| **lock / `thela.lock`** | committed map from goal to accepted artifact hash (D-12) |
| **provider** | an implementation of `SynthProvider` (`anthropic`, `ollama`, `external`, `replay`, `scripted`) |
| **external backend** | a user-chosen program that answers synthesis requests with IR over the `compiler/22` §3.2 protocol (D-42) |
| **replay fixture** | recorded prompt/response pair served by the `replay` provider |
| **verification** | the pipeline a candidate IR must pass before it is accepted (`compiler/22`) |
| **budget** | per-goal resource limits; `budget` lines only tighten system caps (D-8) |
| **fuel** | deterministic step counter; exhaustion is `BudgetExceeded` |
| **epoch** | Wasmtime's coarse interruption tick driving the wall-clock watchdog |
| **host function** | function the runtime exposes to WASM modules; allowlisted (INV-4) |
| **reference interpreter** | `thela-interp`; defines execution semantics (P-4) |
| **differential test** | same IR + inputs on interpreter and WASM, results must match |
| **golden test** | snapshot of a deterministic output (AST, diagnostics, IR, trace) |

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-ERR-01 | Every code in §2 exists in `thela-diagnostics` with the listed name, and no code there is missing from §2. |
| AC-ERR-02 | Using each §4 reserved word as an identifier yields `TL0104`. |
| AC-ERR-03 | Every rendered diagnostic ends its first line with `[TLnnnn]` and its `--json` form carries the same code. |
