# Velme — Code conventions

**Covers:** how Rust code in `crates/`, `xtask/`, `fuzz/` and `benches/` is written, where the spec doesn't fix it.
Complements `CONTRIBUTING.md` §Writing code (not repeated here).
**Read when:** writing or reviewing code. Rules carry `CC-*` ids so reviews can cite them.

---

## 1. Constants: one definition, referenced by name

**CC-CONST-01** A value that carries meaning — a diagnostic code, keyword, reserved word, builtin name, IR node tag,
version string, budget default, config key, env var name — is defined **once** and referenced by name everywhere
else, tests included. Never retype the literal: a typo in a retyped `"VL0204"` compiles and fails silently.

**CC-CONST-02** "Once" means one owner, not one global constants module. A constant lives in the crate that owns the
concept (below). A crate that needs another crate's constant depends on that crate as INV-9 already allows — never
copies it.

**CC-CONST-03** A closed set is an `enum` (exhaustive `match`, no `_ =>` arm on it inside its owning crate); its
wire/spelling form comes from one `as_str()`/`serde(rename)` mapping on the enum, not from literals at call sites.

| Kind | Form | Home | Example |
|---|---|---|---|
| Diagnostic codes (`reference/90`) | `enum Code` with `as_str() -> "VL0204"` and the default message | `velme-diagnostics` | `Code::TypeMismatch` |
| Keywords + reserved words (`language/10`, D-24) | `enum Keyword` | `velme-syntax` | `Keyword::Goal`, `Keyword::is_reserved()` |
| Builtin names + signatures (`language/14`) | `enum Builtin` with a signature table | `velme-builtins` | `Builtin::Maximum` |
| IR node tags (`compiler/21`) | serde enum tag on the IR types; schema derived (schemars) | `velme-ir` | `Node::FieldGet { .. }` |
| Version strings (language, IR, builtins, prompt) | `pub const` per crate | owning crate (`velme-syntax`, `velme-ir`, `velme-builtins`, `velme-synth`) | `velme_ir::IR_VERSION` |
| Budget system caps (`runtime/30` §7, D-77) | `pub const` in `limits` | `velme-builtins` | `velme_builtins::limits::MAX_CALL_DEPTH` |
| Config keys, env var names (`tooling/40`) | `pub const` beside the config struct | `velme-cli` (`velme-synth` for provider keys) | `env::API_KEY` |
| Failure kinds (`runtime/30`) | `enum FailureKind` | `velme-runtime` | `FailureKind::BudgetExceeded` |

## 2. Errors and diagnostics

**CC-ERR-01** No `unwrap`, `expect`, `panic!`, `unreachable!`, indexing or slicing that can panic on data derived from
user source, JSON input, IR from a provider, lock files or artifacts. Those paths return `Result`. `expect` is allowed
only for true internal invariants, with a message naming the invariant. Clippy enforces
`unwrap_used`/`expect_used`/`indexing_slicing` as warnings (→ errors in the gate) in non-test code.

**CC-ERR-02** User-facing problems are `Diagnostic`s (code + span + friendly message + optional help/notes), not
`Error` strings. Internal/library errors use `thiserror` enums per crate; `anyhow` only in `velme-cli` and `xtask`.

**CC-ERR-03** Friendly messages (P-6) are written for a learner: say what was expected and what was found, point at
the span, suggest a fix when one is known. No Rust type names, no "IR", no "WASM" in beginner-facing text; those go in
notes shown with `--verbose`.

**CC-ERR-04** Report every independent error in one pass (parser recovery, sema continues past the first mismatch).
Don't cascade: a node that already failed is marked erroneous and suppresses follow-on diagnostics.

## 3. Determinism (INV-3)

**CC-DET-01** Anything that can reach a result, trace, fingerprint, artifact, snapshot or diagnostic order iterates
in a defined order: `BTreeMap`/`BTreeSet`, `IndexMap`, or an explicit sort. `HashMap`/`HashSet` are fine for lookups
only.

**CC-DET-02** No `SystemTime`, `Instant`, thread ids, `rand` or environment reads in `velme-sema`, `velme-ir`,
`velme-check`, `velme-builtins`, `velme-interp`. Time enters only the runtime watchdog and telemetry, and is never part of
a result or a fingerprint.

**CC-DET-03** All `Number` work goes through the one decimal `Number` type in `velme-builtins` (D-36). No `f64` holds a
language value anywhere — including JSON decoding, which reads number text exactly (serde_json `arbitrary_precision`
or equivalent).

**CC-DET-04** Concurrency never decides an outcome: results are collected by binding index, and failures are chosen
by source order (D-9).

## 4. Crate structure and APIs

**CC-API-01** Default to private; `pub(crate)` for crate internals; `pub` only for the crate's stated API
(`compiler/20`). A new `pub` item in a core crate is an API decision — mention it in the gate report.

**CC-API-02** Dependency direction is INV-9. Adding a workspace dependency edge not in `compiler/20`'s table needs
architect-review. Adding a third-party crate: prefer ones already in the workspace; justify new ones in the PR/gate
report (purpose, maintenance, license passes `cargo-deny`).

**CC-API-03** Vendor SDK/HTTP types for LLM providers live only in `velme-synth`'s provider modules; the trait and its
request/response types are Velme types (INV-7).

**CC-API-04** `#![forbid(unsafe_code)]` in every crate except `velme-wasm`, where each `unsafe` block carries a
`// SAFETY:` comment.

**CC-API-05** Spans are carried from the AST through sema into the IR's source map so every runtime failure can point
at source (`runtime/30` trace). Don't drop a span to simplify a signature.

## 5. Style

**CC-STY-01** `rustfmt` defaults (repo `rustfmt.toml`), clippy `-D warnings` with the workspace lint table in the root
`Cargo.toml`. Edition and toolchain pinned in `rust-toolchain.toml`.

**CC-STY-02** Names follow the spec's vocabulary (`reference/90` glossary): `Goal`, `Binding`, `Wave`, `Tail`,
`Artifact`, `Fingerprint` — not synonyms (`Function`, `Step`, `Stage`).

**CC-STY-03** Doc comments (`///`) on every `pub` item: one line on what it is, plus the spec id it implements.
No module-level essays; the spec is the essay.

## 6. Tests

**CC-TEST-01** Test fn names start with the criterion id in snake case: `ac_chk_05_nullable_field_needs_narrowing`.
A test covering several criteria names the main one and lists the rest in a one-line comment.

**CC-TEST-02** Golden snapshots (`insta`) for token streams, ASTs, diagnostics (rendered text and `--json`), IR and
traces. Snapshot inputs live beside the test as `.velme` files under `tests/golden/<area>/`; the snapshot is the
reviewable output.

**CC-TEST-03** Every rejecting rule has a test asserting the exact `Code`, not just "is error".

**CC-TEST-04** LLM-dependent tests use the `scripted` provider (inline IR) or `replay` fixtures in
`tests/fixtures/synth/`. Recording a new replay fixture is an explicit, reviewed step; live runs are opt-in only (D-13).

**CC-TEST-05** Property tests (`proptest`) for parser round-trips, `Number` rules, canonical JSON and fingerprint
stability; the differential test (interpreter vs WASM) runs over the whole `examples/` and golden IR corpus.
