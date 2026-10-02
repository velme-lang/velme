# Changelog

All notable changes to Velme are recorded here, in the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.
Versions follow semver (`delivery/52` §5). Version bumps that change a fingerprint input note "invalidates locks: yes/no"
(R-REL-07).

## [Unreleased]

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
