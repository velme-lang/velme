# 51 — Testing & Quality Gates

**Status:** v0.1 · **Area:** QA
**Read when:** writing tests, adding a golden file, fuzz target or benchmark, or running the local quality gate.
**Depends on:** [SPEC](../SPEC.md), [50-roadmap](50-roadmap.md), [22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md), all specs (each owns its `AC-*`)

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
| **Fuzz** | targets `parse` (parser), `validate` (IR validator, arbitrary JSON), `differential` (typed valid IR + inputs on both backends, M7) | `cargo-fuzz` in `fuzz/`, on a pinned nightly toolchain | 60 s smoke per target in a CI job; the stable test `ac_qa_06_*` replays the committed corpora and checks that the workflow lists every target (D-118) |
| **Differential** | interpreter vs WASM on the same IR + inputs: equal value, equal full diagnostic, equal fuel and memory used (`runtime/31` R-SBX-15) | `velme-test-support` harness and typed valid-IR generator, shared by proptest and the fuzz target | all golden IR + examples + generated IR (M7, D-118) |
| **Integration** | CLI end-to-end on `examples/` using the `replay` provider and committed locks | `assert_cmd` + `insta` | covers the AC-RDM set |
| **Examples-as-tests** | every file in `examples/` passes `velme test --locked` | xtask step | an example that stops working fails the gate |
| **Live LLM** | real provider synthesis of the success-criteria programs | by hand through the CLI against `anthropic`, recorded into a scratch directory | a manual gate step only (D-13): the gate report holds the transcripts, the model version and the lock, nothing is committed, and there is no live test target in v0.1 (D-148) |
| **Benchmarks** | compile time, interpreter throughput, scheduler overhead, WASM compile + run | `criterion` (default features off) in a `benches/` folder inside each crate it measures, from M8 (D-130) | report-only and not part of PR CI; no nightly tracking before v0.1.0-alpha (D-132). The §6 targets are asserted by ignored release-mode tests `ac_cmp_07_*` and `ac_qa_07_*`, each taking the best of N runs, which `cargo xtask gate` runs (R-QA-07, D-130). M7 has one ignored release-mode test that prints fuel/s for both backends: the number goes in the gate report, and it fails only if the maximum budget would take over 60 s (D-51, D-118) |

**R-QA-03** Golden snapshots are updated only with `cargo insta review` (or `INSTA_UPDATE=always` for a bulk rename),
and every snapshot change appears in the PR diff for review. Never hand-edit a `.snap` file.
**R-QA-04** Tests that need synthesized IR use the `replay` provider with fixtures under `tests/fixtures/synth/`, or the
`scripted` provider for unit tests. A test that silently falls through to a real provider is a bug.
**R-QA-05** Every diagnostic code in `reference/90` has at least one golden test that triggers it and snapshots its
human and JSON rendering.
**R-QA-06** Every bug fix adds a regression test named for the issue (`regression_gh_123_…`) or the AC it violated.
**R-QA-09** Blessing fixtures with `VELME_BLESS_FIXTURES=1` takes a process-wide lock around the bless step, so tests running in parallel cannot race on the same files.

## 3. Determinism tests (INV-3)

| Test | Assertion |
|---|---|
| repeat-run | 100 runs of each success-criteria program: byte-identical result JSON and trace (excluding durations) |
| parallel-vs-sequential | scheduler with 1 worker vs N workers: identical result, trace, failure code (D-9) |
| cross-backend | interpreter vs WASM: identical value, full diagnostic, fuel and memory used (differential layer, D-118) |
| fingerprint stability | fixed program → fixed hash, committed as a snapshot; changes only with a version bump |
| random | `random(seed, index)` reference vectors committed (`language/14`) |
| platform | CI matrix Linux/macOS/Windows produces the same fingerprints and results |

## 4. Quality gate

`cargo xtask verify` is the one local gate (same steps as CI, `delivery/52` §4):

| Step | Command |
|---|---|
| Format | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets --features velme-cli/test-provider -- -D warnings` |
| Lint, default features | `cargo clippy -p velme-cli --all-targets -- -D warnings` (what `cargo install` builds; `test-provider` off) |
| Tests (unit, golden, property, integration, examples) | `cargo test --workspace --all-targets --features velme-cli/test-provider` (nextest optional; D-94), also the macOS and Windows CI jobs |
| Docs build | `cargo doc --workspace --no-deps` with `-D warnings` |
| Dependencies | `cargo deny check` (advisories, licenses, bans, sources) |
| Layering | `xtask` crate-graph check against INV-9 (`delivery/52` R-REL-03) |
| Release features | `cargo tree -e features,normal -p velme-cli` names no `test-endpoint` (`compiler/22` R-SYNTH-44) |
| Coverage audit | `xtask ac-audit` (§5); `--strict` from M8's exit (D-139) |
| Fixture scrub | a test in the default suite finds no key-shaped strings under `tests/fixtures` (R-SEC-07; from M8) |

`cargo xtask verify --quick` runs fmt, clippy and the tests of changed crates for inner-loop use.

**R-QA-07** Every change keeps `cargo xtask verify` green. A phase gate (`delivery/50` R-RDM-02) additionally runs the
fuzz smoke and the benchmark budgets of §6. From M8, `cargo xtask gate` runs the phase-gate steps: the ignored
release-mode `ac_cmp_07_*` and `ac_qa_07_*` tests, and each `ac_rdm_*` test 20 times, each in a fresh process, printing
the pass counts as `delivery/50` R-RDM-04's evidence; `cargo xtask verify` does not run them (D-130).

## 5. Coverage audit

`xtask ac-audit` greps every `AC-[A-Z]+-\d+` defined in `docs/spec/**` (the acceptance tables) and every `#[test]`
function name under `crates/**` and `tests/**`, lowercasing ids. It fails when a criterion has no test, and lists tests
citing unknown criteria, which always fail. Without `--strict` it lists the criteria with no test but does not fail;
from M8's exit `cargo xtask verify` runs it with `--strict`, so a new criterion ships with its test (D-139).

## 6. Performance targets

Measured on a mid-range laptop (4 performance cores), the reference machine, release build, warm file cache. The
workloads are fixed by D-131; cold compile and per-example `velme test` are reported, not asserted.

| Operation | Target |
|---|---|
| `velme check` on a 1,000-line file (a seeded generator's program) | < 100 ms |
| Parse only, 1,000 lines | < 10 ms |
| `velme run --locked`, cache hit, small program (`find_badge`, interpreter and `auto`; CLI start to exit) | < 50 ms |
| Interpreter: a `reduce` over `range(1000)` nested inside a `reduce` over `range(1000)` (1M additions) | < 200 ms |
| WASM leaf: same workload, module already compiled, instantiation and run thread included | ≤ 0.5× interpreter time (D-118); report-only for v0.1 if still missed after M8c (D-125) |
| Scheduler overhead per call node (128 trivial calls minus their leaves alone, ÷ 128) | < 20 µs |
| Fingerprint of a 100-goal generated program | < 5 ms |

**R-QA-08** A benchmark regression over 20 % against the stored baseline fails the nightly job and opens an issue; it
does not block PRs. Deferred until after v0.1.0-alpha, with the nightly job and its baselines; until then the gate's
numbers go in the gate report (D-132).

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-QA-01 | `cargo xtask verify` runs all §4 steps and exits non-zero if any step fails. |
| AC-QA-02 | The default test suite passes with networking disabled and no API key set. |
| AC-QA-03 | `xtask ac-audit` fails when an `AC-*` id is added to a spec with no matching test. |
| AC-QA-04 | Every `VLnnnn` code in `reference/90` is triggered by a golden test. |
| AC-QA-05 | The determinism tests in §3 pass on Linux, macOS and Windows. |
| AC-QA-06 | Each fuzz target runs a 60-second smoke without crash in the pinned-nightly CI job; on stable, `ac_qa_06_*` replays the committed corpora without crash and fails if the workflow omits a target (D-118). |
| AC-QA-07 | §6 targets are met at the M8 gate (the WASM row per D-125). |
