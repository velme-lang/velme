# 51 — Testing & Quality Gates

**Status:** v0.1 · **Area:** QA
**Read when:** writing tests, adding a golden file, fuzz target or benchmark, or running the local quality gate.
**Depends on:** [SPEC](../SPEC.md), [50-roadmap](50-roadmap.md), [22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md), all specs (each owns its `AC-*`)
**Source:** §23, §43.4 (golden, benchmarks, fuzz), §43.7, §52

## 1. Purpose & boundaries

Defines how the Velme implementation is tested and what "green" means. Testing **generated programs** (examples,
generated inputs, check runner) is owned by `compiler/22`; this file covers testing **the implementation**.

**R-QA-01** Every `AC-*` in every spec maps to at least one automated test whose name starts with the criterion id:
`ac_goal_03_self_call_is_rejected`, or `#[test] fn ac_rdm_05_cycle_fails()`. The coverage audit (§5) fails on any
unmapped criterion.
**R-QA-02** The default test suite needs no network, no API key and no Docker. It is deterministic: no wall-clock
assertions, no unseeded randomness, no dependence on test execution order.

## 2. Test layers

| Layer | Scope | Tooling | Rules |
|---|---|---|---|
| **Unit** | lexer rules, type rules, IR validator rules, builtins, fingerprints, scheduler waves | `cargo test` in each crate | pure, no I/O except temp dirs |
| **Golden** | parser → AST JSON; diagnostics → rendered text + JSON; source → IR; `explain`/`trace` output | `insta` snapshots, inputs under `tests/golden/<kind>/*.velme` | R-QA-03 |
| **Property** | parser round-trip (print → parse), canonical-JSON stability, type-checker never panics, interpreter determinism | `proptest` with fixed seeds in CI | failing seeds committed as regression cases |
| **Fuzz** | parser, IR validator (arbitrary JSON), runtime boundary (arbitrary valid IR + inputs) | `cargo-fuzz` in `fuzz/` | 60 s smoke per target on PR; long runs nightly |
| **Differential** | interpreter vs WASM on the same IR + inputs: equal result, equal failure code, equal fuel class | `velme-test-support` harness | all golden IR + proptest-generated IR (M7) |
| **Integration** | CLI end-to-end on `examples/` using the `replay` provider and committed locks | `assert_cmd` + `insta` | covers the AC-RDM set |
| **Examples-as-tests** | every file in `examples/` passes `velme test --locked` | xtask step | an example that stops working fails the gate |
| **Live LLM** | real provider synthesis of the success-criteria programs | `VELME_LIVE_LLM=1 cargo test -p velme-synth --test live` | opt-in only (D-13); records fixtures with `--record` |
| **Benchmarks** | compile time, interpreter throughput, scheduler overhead, WASM compile + run | `criterion` in `benches/` | tracked nightly; not a pass/fail gate except §6 budgets |

**R-QA-03** Golden snapshots are updated only with `cargo insta review` (or `INSTA_UPDATE=always` for a bulk rename),
and every snapshot change appears in the PR diff for review. Never hand-edit a `.snap` file.
**R-QA-04** Tests that need synthesized IR use the `replay` provider with fixtures under `tests/fixtures/synth/`, or the
`scripted` provider for unit tests. A test that silently falls through to a real provider is a bug.
**R-QA-05** Every diagnostic code in `reference/90` has at least one golden test that triggers it and snapshots its
human and JSON rendering.
**R-QA-06** Every bug fix adds a regression test named for the issue (`regression_gh_123_…`) or the AC it violated.

## 3. Determinism tests (INV-3)

| Test | Assertion |
|---|---|
| repeat-run | 100 runs of each success-criteria program: byte-identical result JSON and trace |
| parallel-vs-sequential | scheduler with 1 worker vs N workers: identical result, trace, failure code (D-9) |
| cross-backend | interpreter vs WASM: identical result and failure code (differential layer) |
| fingerprint stability | fixed program → fixed hash, committed as a snapshot; changes only with a version bump |
| random | `random(seed, index)` reference vectors committed (`language/14`) |
| platform | CI matrix Linux/macOS/Windows produces the same fingerprints and results |

## 4. Quality gate

`cargo xtask verify` is the one local gate (same steps as CI, `delivery/52` §4):

| Step | Command |
|---|---|
| Format | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Tests (unit, golden, property, integration, examples) | `cargo test --workspace --all-targets` (nextest optional) |
| Docs build | `cargo doc --workspace --no-deps` with `-D warnings` |
| Dependencies | `cargo deny check` (advisories, licenses, bans, sources) |
| Layering | `xtask` crate-graph check against INV-9 (`delivery/52` R-REL-03) |
| Coverage audit | `xtask ac-audit` (§5) |
| Fixture scrub | no key-shaped strings under `tests/fixtures` (R-SEC-07) |

`cargo xtask verify --quick` runs fmt, clippy and the tests of changed crates for inner-loop use.

**R-QA-07** Every change keeps `cargo xtask verify` green. A phase gate (`delivery/50` R-RDM-02) additionally runs the
fuzz smoke and the benchmark budgets of §6.

## 5. Coverage audit

`xtask ac-audit` greps every `AC-[A-Z]+-\d+` defined in `docs/spec/**` (the acceptance tables) and every test function
name under `crates/**` and `tests/**`, lowercasing ids. It fails when a criterion has no test, and lists tests citing
unknown criteria. Criteria owned by a later phase are allowed to be missing only while that phase's row in
`docs/plan/mvp-plan.md` is `TODO`.

## 6. Performance targets

Measured on a mid-range laptop (4 performance cores), release build, warm file cache.

| Operation | Target |
|---|---|
| `velme check` on a 1,000-line file | < 100 ms |
| Parse only, 1,000 lines | < 10 ms |
| `velme run --locked`, cache hit, small program (CLI start to exit) | < 50 ms |
| Interpreter: `reduce` over 1M numbers | < 200 ms |
| WASM leaf: same workload | ≤ 0.5× interpreter time, excluding first compile |
| Scheduler overhead per call node | < 20 µs |
| Fingerprint of a 100-goal program | < 5 ms |

**R-QA-08** A benchmark regression over 20 % against the stored baseline fails the nightly job and opens an issue; it
does not block PRs.

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-QA-01 | `cargo xtask verify` runs all §4 steps and exits non-zero if any step fails. |
| AC-QA-02 | The default test suite passes with networking disabled and no API key set. |
| AC-QA-03 | `xtask ac-audit` fails when an `AC-*` id is added to a spec with no matching test. |
| AC-QA-04 | Every `VLnnnn` code in `reference/90` is triggered by a golden test. |
| AC-QA-05 | The determinism tests in §3 pass on Linux, macOS and Windows. |
| AC-QA-06 | Each fuzz target runs a 60-second smoke without crash in CI. |
| AC-QA-07 | §6 targets are met at the M8 gate. |
