# 22 — Spellbook: Synthesis & Verification

**Status:** v0.1 · **Area:** SYNTH
**Read when:** touching the LLM provider interface, a provider, the prompt, the retry loop, the verification pipeline, generated test inputs, or synthesis cost/config.
**Depends on:** [SPEC](../SPEC.md), [20-compiler-architecture](20-compiler-architecture.md), [21-ir](21-ir.md), [runtime/32](../runtime/32-artifacts-cache.md), [tooling/41](../tooling/41-security-privacy.md)
**Source:** §3.1, §3.2, §20, §21, §22, §23, §42 (LLM)

## 1. Purpose & boundaries

Spellbook turns one goal's typed HIR into a **verified** IR artifact. It owns: the provider-neutral interface, the
prompt contract, structured output, the retry loop, the verification pipeline and generated test inputs. It does not
own IR validity (that is [21](21-ir.md) §6), execution semantics ([runtime/30](../runtime/30-execution-vibevm.md)) or
the artifact store and lockfile ([runtime/32](../runtime/32-artifacts-cache.md)).

Principle: the LLM proposes, the deterministic pipeline disposes (INV-1, INV-2).

## 2. When synthesis happens

| Goal kind (20 §3 phase 6) | What is synthesized | LLM call? |
|---|---|---|
| `leaf` | the whole `body` | yes, on cache miss |
| `composite` | the tail `body` over inputs + call bindings (D-5) | yes, on cache miss |
| `wired` (D-4) | nothing — compiler IR | never |

**R-SYNTH-01** Goals are built in post-order of the goal DAG: a composite goal is synthesized only after every child
has an accepted artifact, because verification executes the real children (§6).
**R-SYNTH-02** Lookup order before any provider call: (1) `thela.lock` entry whose `contract_key` matches (runtime/32 §5), (2) artifact
store by `synthesis_key` (D-11, runtime/32 §2), (3) provider. A hit at (1) or (2) makes no network call.
**R-SYNTH-03** `--locked` and `--offline` never construct a provider (20 R-CMP-19). A goal that would need synthesis
fails with `TL0702 LockStale` (`--locked`) or `TL0404 ProviderUnavailable` (`--offline`).

## 3. Provider interface (D-13, D-14, INV-7)

```rust
#[async_trait]
pub trait SynthProvider: Send + Sync {
    fn id(&self) -> &str;                    // "anthropic" | "replay" | "scripted" | …
    fn model(&self) -> &str;                 // provider-reported model id → manifest model_version
    async fn complete(&self, prompt: &SynthPrompt, limits: &SynthLimits)
        -> Result<SynthReply, ProviderError>;
}

pub struct SynthPrompt { pub prompt_version: String, pub system: String, pub turns: Vec<Turn>,
                         pub output_schema: serde_json::Value }          // IR JSON Schema (21 R-IR-20)
pub struct SynthReply  { pub ir_json: String, pub usage: Usage, pub latency: Duration }
pub enum  ProviderError { NotConfigured, Unavailable(String), RateLimited { retry_after: Option<Duration> },
                          Refused(String), Timeout, Malformed(String) }
```

**R-SYNTH-04** The trait and its types are plain Rust + `serde_json`. Vendor SDK/HTTP types never appear in a
signature; each provider is a module in `thela-synth` behind a Cargo feature (`provider-anthropic`).
**R-SYNTH-05** Providers:

| Provider | Use | Behaviour |
|---|---|---|
| `anthropic` | the MVP's real provider (D-14) | Messages API, output constrained to the IR JSON Schema (tool/structured output), temperature 0 |
| `replay` | integration tests, golden builds, CI | reads `tests/fixtures/synth/<synthesis-key>.json` (prompt hash + ordered replies); missing fixture → `TL0404` naming the key; `THELA_SYNTH_RECORD=1` with a live provider writes fixtures |
| `scripted` | unit tests of the retry loop and pipeline | in-memory queue of replies/errors |

**R-SYNTH-06** Live-provider tests run only with `THELA_LIVE_LLM=1` and are never part of the default gate (D-13).
**R-SYNTH-07** `ProviderError` mapping: `NotConfigured` → `TL0405`; `Unavailable`/`Timeout`/`RateLimited` after
transport retries → `TL0404`; `Refused`/`Malformed` count as a failed attempt (§5).

## 4. Prompt contract (§20.1)

The prompt is rendered from a versioned template in `crates/thela-synth/prompts/`. `prompt_version` =
template id + BLAKE3 of the template bytes, so any edit changes synthesis keys (D-11).

| Section | Contents | From |
|---|---|---|
| Header | `prompt_version`, `ir_version`, `builtins_version`, task kind (`leaf` / `composite-tail`) | constants |
| Goal | signature `Name(params) -> Output` | HIR |
| Types | every reachable record type with fields | HIR |
| Locals | composite only: each call binding `name: Type = Child(args)` — child **signatures** only, never child IR | HIR (D-5, D-11) |
| Plan | normalized plan text (D-21), fenced and labelled as untrusted user description (§9) | HIR |
| Checks | source text of each check + its lowered form | HIR / `thela-check` |
| Examples | each `examples:` item with literal values | HIR (D-7) |
| Budget | effective budget (runtime/30 §7) | HIR + system caps |
| Allowed builtins | name + signature of each catalog entry | `thela-builtins` |
| Output contract | "return one IR goal JSON matching the schema; omit `calls`; use only listed builtins" + the schema | `thela-ir` |

**R-SYNTH-08** The prompt contains nothing outside this table: no file paths, no other goals' plans, no environment,
no user identity (tooling/41).
**R-SYNTH-09** Output is constrained to the IR JSON Schema where the provider supports it; Thela still runs its own
full validator on every reply (§20.2) — a provider's schema guarantee is never trusted.
**R-SYNTH-10** The reply is the IR only. Any prose, markdown fence or partial JSON is a failed attempt with `TL0401`.

## 5. Retry loop (§21)

```
attempt 0 ─► validate (21 §6) ─► verify (§6) ─► accepted
     ▲             │ fail              │ fail
     └── diagnostics appended as a new turn (max_retries = 3)
```

**R-SYNTH-11** On a validation or verification failure, the diagnostics (code, message, JSON path, and for check
failures the input, the assertion and the actual values) are appended as a new turn and the provider is asked again.
At most `max_retries` (default and cap: 3) retries follow the first attempt.
**R-SYNTH-12** Transport errors (`RateLimited`, `Unavailable`, `Timeout`) are retried with exponential backoff up to 2
times per attempt and do not consume a synthesis retry.
**R-SYNTH-13** After the last retry the goal fails with `TL0403`: "Thela could not build this goal." plus the last
attempt's diagnostics in `--verbose`. The source `plan` is never modified; nothing is written to the artifact store
or the lock.

## 6. Verification pipeline (§22)

A candidate becomes an artifact only after every step passes, in order:

| Step | Check | Failure |
|---|---|---|
| 1–7 | IR validation stages (21 §6): schema, structure, names, types, capabilities, call graph, resources | `TL0401`/`TL0402`/`TL0801` |
| 8a | every `examples:` item runs on the interpreter and matches (D-7) | `TL0502` |
| 8b | every generated input (§7) executes within budget | `TL06xx` |
| 9 | all `check`s hold for every example and generated input | `TL0501` |

**R-SYNTH-14** Verification executes on the reference interpreter with the goal's effective budget; composite goals
run their real, already-accepted children (R-SYNTH-01). A `Timeout` (D-10) is re-run once; a second `Timeout` counts
as a failed attempt but is never recorded as a cached outcome.
**R-SYNTH-15** Any failure in steps 8–9 wraps as `TL0503 VerificationFailed` with the first failing case (examples
before generated inputs, then generation order) as the primary cause.
**R-SYNTH-16** Only a fully verified candidate is eligible for the artifact store (§22, runtime/32 R-ART-11).

## 7. Generated test inputs (§23)

The `check` block is a property, not a proof: it is evaluated over a bounded, deterministic input set.

| Source | Values |
|---|---|
| 1. Examples | the goal's `examples:` inputs, in source order |
| 2. Boundary | per type: `Number` {0, 1, −1, 0.5, 1e6, −1e6}; `Text` {"", "a", "Lina", "é🙂"}; `Boolean` {true, false}; `T?` {nothing, each `T` boundary}; `List<T>` {[], [x], [x, y, z], [x, x]} |
| 3. Small | combinations of small values across fields/params (pairwise, not full product) |
| 4. Pseudo-random | SplitMix64 stream seeded from the goal's `contract_key`; lists ≤ 8 items, text ≤ 16 chars, numbers integer-valued in [−1000, 1000] half the time |

**R-SYNTH-17** Input generation is a pure function of the goal's `contract_key` (runtime/32 §2), so changing model or prompt does not change it: the same goal always gets the
same inputs on every machine (INV-3).
**R-SYNTH-18** Caps: ≤ 64 inputs per goal (examples always included, never dropped), ≤ 256 KiB per serialized input.
**R-SYNTH-19** A check failure on a generated input is shown to the learner as a counterexample ("your check fails
when `players` is `[]`"), so they can refine the check; the message suggests the `if … then …` rewrite
(`if price >= 0 then result >= 0`, D-6). An `assume:` precondition block is Future; the word is reserved (D-39).

## 8. Configuration & secrets

```toml
# thela.toml (project root; all keys optional)
[synth]
provider          = "anthropic"
model             = "<model id>"      # or THELA_MODEL; never a code constant (D-14)
max_retries       = 3                 # 0..=3
timeout_secs      = 60
max_output_tokens = 8192
max_calls_per_build = 50              # hard stop across the whole build
```

**R-SYNTH-20** API keys come only from the environment (`ANTHROPIC_API_KEY`). A key-like value in `thela.toml` is an
error. Keys never appear in logs, traces, fixtures, artifacts, diagnostics or `--verbose` output; the replay recorder
stores no headers.
**R-SYNTH-21** `max_calls_per_build` bounds cost: when reached, remaining goals fail with `TL0403` and a message
naming the limit. The build summary reports calls, tokens in/out and cache hits.

## 9. Untrusted plan text

**R-SYNTH-22** Plan, check and example text are untrusted data: they are fenced in the prompt and the instructions
state they describe intent only. Thela does not rely on this for safety — whatever the LLM returns must still pass
validation and runs sandboxed with no capabilities (INV-1, INV-4, tooling/41).

## 10. Local synthesis log

**R-SYNTH-23** Each attempt appends one JSON line to `.thela/synth-log.jsonl`: time, goal, synthesis key, provider,
model, attempt, outcome code, tokens, latency. Plan text and values are excluded by default. The log never leaves the
machine (tooling/41) and is git-ignored.

## 11. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SYNTH-01 | With a matching lock entry or store hit, a build makes zero provider calls (asserted with a panicking provider). |
| AC-SYNTH-02 | `scripted` provider returning invalid IR twice then valid IR: build succeeds after 2 retries; each retry turn contains the previous diagnostics. |
| AC-SYNTH-03 | Four consecutive invalid replies → `TL0403`; no artifact written, lock unchanged, source unchanged. |
| AC-SYNTH-04 | A reply with a `call` node fails validation and the retry turn cites `TL0402` (INV-6). |
| AC-SYNTH-05 | A candidate that passes validation but fails an example is rejected with `TL0502` and never cached. |
| AC-SYNTH-06 | Generated inputs for a fixed goal are byte-identical across two runs and two OSes. |
| AC-SYNTH-07 | The rendered prompt for a golden goal matches its snapshot and contains no child IR, file paths or env values. |
| AC-SYNTH-08 | `--locked` with a stale entry fails with `TL0702`; `--offline` with a cache miss fails with `TL0404`; neither constructs a provider. |
| AC-SYNTH-09 | No API key string appears in any output, log, fixture or artifact after a recorded build (grep test with a sentinel key). |
| AC-SYNTH-10 | `replay` provider reproduces a recorded build byte-for-byte (same artifacts, same lock). |
