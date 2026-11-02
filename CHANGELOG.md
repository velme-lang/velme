# Changelog

All notable changes to Velme are recorded here, in the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.
Versions follow semver (`delivery/52` §5). Version bumps that change a fingerprint input note "invalidates locks: yes/no"
(R-REL-07).

## [Unreleased]

## [0.2.0-alpha.1] - 2026-10-26

Coordinated language break to `velme/0.2`, IR `0.2`, request `0.2` and builtins `0.2` (R-REL-14, D-160, D-168). Invalidates locks: yes.

This release brings input domains ([RFC 0001](rfc/0001-input-domains.md)) and the `pow` built-in.

### Added

- Input domains: `type Name = Base where <condition>` and record domains `type Name where <cond>:` with deep predicate checking, test value generator, scarcity probing, and diagnostics `VL0209`, `VL0210`, and `VL0504`.
- IR `0.2` schema and prompt request `0.2` capturing input domain constraints and declarations.
- Builtins `0.2`: value built-in `pow(x, n)` computing $x^n$ exactly with single half-to-even rounding (R-TYP-04), variable fuel based on bit length of $|n|$, integer-valued $|n| \le 1000$, and overflow or zero-base negative-exponent `VL0602` error (D-168).
- Example `examples/professional/loan_payment.velme` demonstrating loan installment calculation with input domains, check, and `pow`.

### Changed

- Synthesis defaults to reply format `notation` across every provider, `external` included (D-169).
- Supported language version is `velme/0.2` only; headerless files default to `0.2`.
- A `language: velme/0.1` header or `project.language = "velme/0.1"` config setting is rejected with migration help (`VL0106` / `VL0902`).
  - Migration: in source files, change the first line to `language: velme/0.2` (or omit the header); in `velme.toml`, change `project.language = "velme/0.2"` (or omit the setting); rebuild the lockfile with `velme build <file>` or `velme run --build <file>`.
- Existing `0.1` locks are stale and invalidated under 0.2.

### Removed

- Support for `velme/0.1` headers, IR `0.1` schemas, and builtins `0.1`.
- `reply_format = "compact"` (D-170): use `notation`, the default; `compact` is now an unknown value (`VL0902`). `prompt_version` changes once, so synthesis keys change.

## [0.1.0-alpha.1] - 2026-10-02

The first release, with language `velme/0.1`, IR `0.1` and builtins `0.1` (D-146). Invalidates locks: no (the keys
hold only the compiler's MAJOR.MINOR; committed artifacts are re-blessed with the new `compiler_version`).

### Added

- The workspace and `cargo xtask verify`, the one local gate, with the crate layering check and the coverage audit
  (M0).
- `velme check`: parsing with error recovery, name and type checking, the call graph and its cycles, with friendly
  diagnostics (`VLnnnn` codes) in text and in a `--json` envelope (M1, M2).
- The Velme IR, its JSON Schema and canonical JSON; the IR validator; decimal `Number`, the builtins and `random`; the
  reference interpreter with checks and examples; content-addressed artifacts and `velme.lock` (M3).
- The VibeVM runtime: goals that don't depend on each other run in parallel with results in source order, fuel, call,
  depth and size budgets, `velme trace` and `velme explain` (M4).
- `velme build`: synthesis through the `anthropic`, `ollama` and `external` providers, with retries fed by
  diagnostics, verification against each goal's examples and generated inputs, and the `replay` provider for
  offline builds; an unchanged goal is never synthesized again (M5).
- `--locked` and `--offline`, `velme artifact`, `velme gc`, `velme cache clean`, `velme.toml` and the user config,
  `--input`/`--arg`, stable exit codes and the local synth log (M6).
- The WASM backend in a Wasmtime sandbox with no files, network or clock, chosen with `--backend interp|wasm|auto`
  and checked against the interpreter by a differential test and fuzzing (M7).
- `-v` phase timings; notes about a bug in Velme's WASM backend in the `--json` envelope's `notices[]` (M8).
- Every example commits its `velme.lock` and `.velme/artifacts`, so `velme run` and `velme test --locked` work on the
  examples with no API key; the README has a quick start (M8).
