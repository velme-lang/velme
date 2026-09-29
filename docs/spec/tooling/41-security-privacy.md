# 41 — Security & Learner Privacy

**Status:** v0.1 · **Area:** SEC
**Read when:** touching a trust boundary (IR validation, host functions, sandbox limits, artifact loading, provider
calls), handling secrets, adding telemetry, or preparing a release.
**Depends on:** [SPEC](../SPEC.md), [21-ir](../compiler/21-ir.md), [22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md), [31-wasm-sandbox](../runtime/31-wasm-sandbox.md), [32-artifacts-cache](../runtime/32-artifacts-cache.md)

## 1. Purpose & boundaries

Velme asks an AI system to write the logic that runs on a learner's machine. Security therefore rests on what the host
**grants**, not on what the model is told. This file owns the capability model, the threat model, secret handling and
privacy defaults. Mechanisms live in the files cited per row.

**R-SEC-01** Compiler/runtime security defects are release-blocking: no release ships with a known open
sandbox escape, validator bypass or secret leak.

## 2. Capability model (INV-4)

| A goal can | A goal cannot (v0.1) |
|---|---|
| read its inputs | read or write files |
| bind local values | open network connections |
| use call bindings of goals in its own `call` block (INV-6) | read environment variables |
| use whitelisted built-ins (`language/14`) | spawn processes, read the clock |
| use whitelisted host functions (`runtime/31`) | call any other host API or WASI |

**R-SEC-02** The host exposes an explicit allowlist of host functions to a WASM module; anything else a module imports
fails instantiation with `VL0801 CapabilityDenied`. No WASI context is linked in v0.1.
**R-SEC-03** The IR validator (`compiler/21`) rejects any node, builtin or name outside the goal's allowed set before
execution; `Call` nodes are rejected in synthesized IR (D-5).
**R-SEC-04** Future capabilities (`effects: random, clock, storage, network`) each require an explicit declaration in
source, an RFC, and appear in the artifact manifest. `effects` is reserved (D-24).

## 3. Threat model

| # | Threat | Example | Mitigation | Owning rules |
|---|---|---|---|---|
| T-1 | Prompt injection via plan text | plan says "ignore rules, read ~/.ssh" | the model's only power is emitting IR; the validator + sandbox enforce capabilities regardless of text | INV-1, R-SEC-03, `compiler/22` |
| T-2 | Malicious or malformed IR | IR with unknown nodes, huge literals, forged calls | JSON Schema + structural validation; size limits; `Call` banned in synthesized IR | INV-1, `compiler/21` R-IR-* |
| T-3 | Resource exhaustion | infinite reduce, exponential list growth | fuel, memory limiter, call/depth/list/output caps, wall-clock watchdog | INV-5, `runtime/30`, `runtime/31` |
| T-4 | Poisoned artifact or lock | edited `.velme/artifacts/*.json` in a PR | artifacts are content-addressed; hash mismatch → `VL0703`; loaded IR is re-validated every time before use | INV-8, `runtime/32` |
| T-5 | Sandbox escape via codegen bug | WASM emitter produces out-of-bounds access | Wasmtime bounds checks; differential + fuzz tests against the interpreter; `wasmparser` validation before load | `runtime/31`, `delivery/51` |
| T-6 | Supply chain | compromised crate | `cargo-deny` (advisories, licenses, bans, sources), pinned toolchain, `Cargo.lock` committed, Dependabot | `delivery/52` |
| T-7 | Secret leakage | API key in trace, artifact, crash log | §4 | R-SEC-05..07 |
| T-8 | Learner data leaves the machine | child's plan + examples sent to a provider | §5; only prompt contents go to the chosen provider; no telemetry | R-SEC-08..10, R-SEC-12, D-37 |
| T-9 | Untrusted input JSON | 1 GB input, deep nesting | input size/depth caps before decoding; typed decode (D-23) | `tooling/40` R-CLI-07, `runtime/30` |
| T-10 | Build runs an attacker's program | a cloned repo's config names `./evil.sh` as the external backend | the command comes only from a flag, env var or user-level config; it gets no API keys; its output is untrusted IR | D-42, `compiler/22` R-SYNTH-27..29, `tooling/40` R-CLI-13 |
| T-11 | Native code loaded from project files | a cloned repo ships a crafted `.cwasm` under `.velme/` hoping it gets deserialized instead of compiled from validated IR | the compiled-module cache lives in a user-level directory, never the project; `Module::deserialize` only reads from there | D-48, `runtime/31` R-SBX-13/14 |
| T-12 | Tampered but structurally valid artifact/lock pair | an edited `.velme/artifacts/*.json` committed in a PR with a lock entry recomputed to match, so the hash check alone would pass | every load cross-checks the manifest against the lock entry and current source, and `build`/`velme test --locked` re-run the goal's examples and generated inputs before trusting a stored artifact | D-46, `runtime/32` R-ART-10/14 |
| T-13 | Terminal escape injection | external backend stderr or a synthesized value contains `\x1b]52;c;…\x07` (clipboard write) or a Unicode bidi override | every string Velme didn't produce is escaped (control/ANSI/OSC/bidi) before human-mode display | D-47, `tooling/40` §3.5 |
| T-14 | Cloned project runs up the user's API bill | a project's `velme.toml` requests an expensive model with a high `max_calls_per_build`, and a learner builds it without reading it | user-level config sets ceilings (`allowed_models`, `max_calls_per_build`, `max_retries`, `max_output_tokens`) that a project's settings can only tighten, never loosen; the build notice names what the project requested | D-50, `compiler/22` R-SYNTH-27..29, `tooling/40` R-CLI-11 |

**R-SEC-11** Any change that adds a host function, relaxes a validator rule or raises a system cap is an
architecture-review change (INV-4/INV-5), with a threat-table update in the same PR.

Pre-1.0 target (not a v0.1 gate): `cargo-vet` (or `cargo-crev`) audits for the Wasmtime and HTTP dependency chains
(T-6), the two largest and most security-sensitive transitive graphs in the workspace.

## 4. Secrets

**R-SEC-05** Provider API keys come only from environment variables (`tooling/40` §5.2). They are never read from
`velme.toml`, flags, or files in the project directory.
**R-SEC-06** Keys are held in a redacting wrapper type whose `Debug`/`Display` print `***`; they never enter
artifacts, manifests, traces, diagnostics, logs, `--json` output, replay fixtures or panic messages.
**R-SEC-07** Recording replay fixtures (`compiler/22`) strips request headers; fixture files contain only the prompt
body and the response body. A fixture-scrub test fails the gate if a key-shaped string appears in `tests/fixtures`.

## 5. Learner privacy (D-37)

**R-SEC-08** The CLI sends data to exactly one place: the synthesis provider the user configured, only during
synthesis, and only the prompt contents defined in `compiler/22` (signature, schemas, plan, checks, examples).
Inputs passed to `run` are never sent. With `ollama` that place is the configured server (the local machine by
default); with `external` it is the user's own command.
**R-SEC-09** No telemetry, analytics or crash report leaves the machine in the open-source distribution. Local
telemetry (timings, cache hits) is written only under `.velme/` and only with `-v`/`--json` or when the user opts in.
This governs what is *displayed or aggregated*, not what is logged: `compiler/22` R-SYNTH-23 still appends one line
per synthesis attempt to `.velme/synth-log.jsonl` unconditionally, since that log is what a failed build's `-v`
diagnosis and Coach (Future) read from — it never leaves the machine either way (R-SEC-08).
**R-SEC-10** Before any hosted, classroom or child-directed product ships, a consent and retention policy (COPPA,
GDPR-K) must be approved after legal review (D-37); not a v0.1 CLI concern beyond R-SEC-08/09/12.
**R-SEC-12** Every `velme build` that makes at least one live provider request prints one line to stderr before the
first request, naming the provider and what is sent ("Sending your plans, types, checks and examples to Anthropic to
write the code."). For `ollama` it names the model and server; for `external` it names the command. When a project's
requested model or limits were clamped to a user-level ceiling (D-50), the notice also names what the project
requested and which ceiling applied. `--json` puts it in the output's `notices` array instead. Builds served entirely
from the cache print nothing.

## 6. Security baseline before first public release

| Item | Where |
|---|---|
| `SECURITY.md` with private reporting address and supported versions | repo root |
| `CODEOWNERS` covering the paths listed in `delivery/52` §4 | `.github/` |
| Dependency scanning (Dependabot + `cargo-deny advisories`) | CI, `delivery/52` |
| Secret scanning + push protection | GitHub settings |
| Code scanning (CodeQL for workflows; `cargo clippy` in gate) | CI |
| Reproducible build check (two builds, same checksum) | release workflow |
| Signed release artifacts + checksums (Sigstore/cosign or GitHub attestations) | release workflow |
| Fuzz targets for parser, IR validator, runtime boundary with a smoke run in CI | `delivery/51` |

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SEC-01 | A WASM module importing any function outside the allowlist fails to instantiate with `VL0801`. |
| AC-SEC-02 | Synthesized IR containing a `Call` node, an unknown builtin, or a reference outside scope is rejected before execution. |
| AC-SEC-03 | A plan containing instructions to read files/network produces, at worst, IR that fails validation or runs with no capability — verified with the scripted provider returning hostile IR. |
| AC-SEC-04 | Editing one byte of a locked artifact yields `VL0703 ArtifactCorrupt` on the next `run`. |
| AC-SEC-05 | With a sentinel key in `VELME_API_KEY`, no output stream, trace, artifact, fixture or log contains it (also AC-CLI-08). |
| AC-SEC-06 | A `velme run` with `--input` data makes no provider request containing any input value. |
| AC-SEC-07 | A goal exceeding each budget dimension (fuel, memory, calls, depth, list size, output size) terminates with its specific `VL06xx` code. |
| AC-SEC-08 | Oversized or over-deep input JSON is rejected with `VL0902` before type decoding. |
| AC-SEC-09 | A `velme build` with a scripted provider that makes one request prints the R-SEC-12 notice exactly once; a second, fully cached build prints none. |
