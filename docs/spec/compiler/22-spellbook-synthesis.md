# 22 — Spellbook: Synthesis & Verification

**Status:** v0.1 · **Area:** SYNTH
**Read when:** touching the provider interface, a provider (LLM or external backend), the synthesis request, the prompt, the retry loop, the verification pipeline, generated test inputs, or synthesis cost/config.
**Depends on:** [SPEC](../SPEC.md), [20-compiler-architecture](20-compiler-architecture.md), [21-ir](21-ir.md), [runtime/32](../runtime/32-artifacts-cache.md), [tooling/41](../tooling/41-security-privacy.md)
**Source:** §3.1, §3.2, §20, §21, §22, §23, §42 (LLM)

## 1. Purpose & boundaries

Spellbook turns one goal's typed HIR into a **verified** IR artifact. It owns: the provider-neutral interface, the
synthesis request and its external protocol, the prompt contract, structured output, the retry loop, the verification
pipeline and generated test inputs. It does not
own IR validity (that is [21](21-ir.md) §6), execution semantics ([runtime/30](../runtime/30-execution-vibevm.md)) or
the artifact store and lockfile ([runtime/32](../runtime/32-artifacts-cache.md)).

Principle: the provider proposes, the deterministic pipeline disposes (INV-1, INV-2). A provider is an LLM
(`anthropic`, `ollama`) or any program speaking the external protocol (§3.2, D-42); the pipeline treats every
candidate the same way, whoever wrote it.

## 2. When synthesis happens

| Goal kind (20 §3 phase 6) | What is synthesized | LLM call? |
|---|---|---|
| `leaf` | the whole `body` | yes, on cache miss |
| `composite` | the tail `body` over inputs + call bindings (D-5) | yes, on cache miss |
| `wired` (D-4) | nothing — compiler IR | never |

**R-SYNTH-01** Goals are built in post-order of the goal DAG: a composite goal is synthesized only after every child
has an accepted artifact, because verification executes the real children (§6).
**R-SYNTH-02** Lookup order before any provider call: (1) `velme.lock` entry whose `contract_key` matches (runtime/32 §5), (2) artifact
store by `synthesis_key` (D-11, runtime/32 §2), (3) provider. A hit at (1) or (2) makes no network call.
**R-SYNTH-03** `--locked` and `--offline` never construct a provider (20 R-CMP-19). A goal that would need synthesis
fails with `VL0702 LockStale` (`--locked`) or `VL0404 ProviderUnavailable` (`--offline`).

## 3. Provider interface (D-13, D-14, D-41, D-42, INV-7)

### 3.1 Trait and providers

```rust
#[async_trait]
pub trait SynthProvider: Send + Sync {
    fn id(&self) -> &str;                    // "anthropic" | "ollama" | "external" | "replay" | "scripted"
    fn model(&self) -> &str;                 // model id (or model+retry_model, R-SYNTH-39), Ollama digest or external backend_version → manifest model_version
    fn input_version(&self) -> &str;         // prompt_version + request options (LLM providers, R-SYNTH-40) or request_version (external)
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits)
        -> Result<SynthReply, ProviderError>;
}

pub struct SynthRequest { pub request_version: String, pub task: TaskKind, pub goal: String,
                          pub signature: Signature, pub types: Vec<RecordType>, pub locals: Vec<LocalBinding>,
                          pub plan: String, pub checks: Vec<CheckItem>, pub examples: Vec<Example>,
                          pub budget: Budget, pub builtins: Vec<BuiltinSig>,
                          pub attempts: Vec<AttemptFeedback>,                // earlier replies + diagnostics (§5)
                          pub output_schema: serde_json::Value }             // reply schema: IR goal or question (R-SYNTH-10)
pub struct SynthReply  { pub reply_json: String, pub usage: Usage, pub latency: Duration }
pub enum  ProviderError { NotConfigured, Unavailable(String), RateLimited { retry_after: Option<Duration> },
                          Refused(String), Timeout, Malformed(String), BackendFailed(String), Pending(String) }
```

`SynthRequest` is the structured form of the §4 table. LLM providers render it into a prompt with the versioned
template (§4); the `external` provider sends it as JSON (§3.2); `replay` and `scripted` ignore it except for keying.

**R-SYNTH-04** The trait and its types are plain Rust + `serde_json`. Vendor SDK/HTTP types never appear in a
signature; each provider is a module in `velme-synth` behind a Cargo feature (`provider-anthropic`).
**R-SYNTH-05** Providers:

| Provider | Use | Behaviour |
|---|---|---|
| `anthropic` | hosted LLM (D-14) | Messages API, output constrained to the IR JSON Schema (tool/structured output), temperature 0 |
| `ollama` | local LLM (D-41) | Ollama chat API at the configured URL, `format` set to the IR JSON Schema, temperature 0, no streaming; no API key |
| `external` | human- or tool-written IR (D-42) | runs the user's command and speaks the §3.2 protocol over stdin/stdout; no network of its own |
| `replay` | integration tests, golden builds, CI | reads `tests/fixtures/synth/<synthesis-key>.json` (prompt hash + ordered replies); missing fixture → `VL0404` naming the key; `VELME_SYNTH_RECORD=1` with a live provider writes fixtures |
| `scripted` | unit tests of the retry loop and pipeline | in-memory queue of replies/errors |

**R-SYNTH-06** Live-provider tests run only with `VELME_LIVE_LLM=1` and are never part of the default gate (D-13).
**R-SYNTH-07** `ProviderError` mapping: `NotConfigured` → `VL0405`; `Unavailable`/`Timeout`/`RateLimited` after
transport retries → `VL0404`; `Refused`/`Malformed` count as a failed attempt (§5); `BackendFailed` → `VL0406`,
not retried; `Pending` → `VL0408`, not retried (R-SYNTH-41).
**R-SYNTH-24** `ollama` resolves the configured model's digest from the server once per build, before any store
lookup, and reports `<model>@<digest>` as `model()`. A tag that now points at different weights therefore changes
`synthesis_key` but never `contract_key` (runtime/32 R-ART-03). A model missing on the server is `NotConfigured`
(`VL0405`, help: `ollama pull <model>`); an unreachable server is `Unavailable` (`VL0404`).
**R-SYNTH-25** Every provider that makes a request is constructed only when a goal misses both the lock and the store
(R-SYNTH-02 step 3), so an `ollama` server or an `external` command is never contacted for a fully cached build.

### 3.2 External backend protocol (D-42)

The `external` provider lets a person or any tool supply the implementation instead of an LLM. Velme starts the
configured command once per message, writes one JSON document to its stdin, closes stdin, and reads one JSON document
from its stdout. Both documents follow the committed `velme-synth-request` JSON Schema, generated from the Rust types
like the IR schema (21 R-IR-20).

| Message | Velme sends | Backend replies |
|---|---|---|
| `describe` | `{"request_version": "0.1", "kind": "describe"}` | `{"backend": "<name>", "backend_version": "<version>"}` |
| `synthesize` | `{"request_version": "0.1", "kind": "synthesize", "request": <SynthRequest>}` | `{"ir": <IR goal>}`, `{"question": "<text>"}`, `{"pending": "<text>"}` or `{"error": "<reason>"}` |

**R-SYNTH-26** `describe` runs once per build; its `backend_version` is `model()` and enters `synthesis_key`, so a new
backend version is a cache miss but never makes a lock stale (runtime/32 R-ART-03).
**R-SYNTH-27** A `synthesize` reply's `ir` is handled exactly like an LLM reply: full validation (21 §6), then
verification (§6). The backend gets no trust the LLM doesn't get (INV-1). A `question` reply follows
R-SYNTH-32..33, as an LLM's does.
**R-SYNTH-28** Non-zero exit, exceeding `external_timeout_secs`, stdout that is not one JSON document, stdout above
2 MiB, or an `{"error"}` reply is `BackendFailed` → `VL0406` naming the goal and backend, with up to 4 KiB of stderr
in the notes. Nothing is written to the store or the lock.
**R-SYNTH-29** The command runs in the project root with the user's own permissions, outside the sandbox (tooling/41
T-10). Its environment is the user's minus every provider API key variable (`tooling/40` §5.2). The command itself
comes only from the `--external-command` flag, `VELME_EXTERNAL_COMMAND` or the user-level config, never from the
project's `velme.toml` (`tooling/40` R-CLI-13).
**R-SYNTH-30** `max_retries` defaults to 0 for `external`, since a deterministic backend returns the same reply
again. When raised, each retry request carries the earlier replies and their diagnostics in `attempts`, as an LLM's
retry turn does (R-SYNTH-11).
**R-SYNTH-41** A `{"pending": "<text>"}` reply means the request is queued for a person or tool and has no answer
yet (D-45). It is `Pending` → `VL0408` for that goal at once, with no retry even when `max_retries` > 0. Other goals
continue. Nothing is written to the store or the lock or cached, so the next build sends the request again. The text
is untrusted and cleaned as in R-SYNTH-33; an empty or over-long text is `BackendFailed` (`VL0406`). It is shown only
as a note, quoted and labelled as the backend's, and as a plain string in `--json`. If an earlier attempt of the same
goal failed in this build, a note gives that attempt's primary diagnostic (R-SYNTH-31). Only `external` replies this
way; the LLM reply schema never includes it (R-SYNTH-10).

*Informative:* a queue backend stores answers under a hash of the `request` document. An edited goal sends a
different request, so an answer is never reused for a goal it wasn't written for.

## 4. Prompt contract (§20.1)

The prompt is rendered from `SynthRequest` (§3) with a versioned template in `crates/velme-synth/prompts/`.
`prompt_version` = template id + BLAKE3 of the template bytes, so any edit changes synthesis keys (D-11). Both LLM
providers share the template. The table below is also the content of `SynthRequest`; the external protocol (§3.2)
sends the same fields as structured JSON.

| Section | Contents | From |
|---|---|---|
| Header | `prompt_version`, `ir_version`, `builtins_version` | constants |
| Output contract | "return one IR goal JSON matching the schema; omit `calls`; use only listed builtins; only if the plan leaves open a choice that changes the result, return `{"question": …}` instead" + the reply schema or its summary (R-SYNTH-35) | `velme-ir` / `velme-synth` |
| Allowed builtins | name + signature of each catalog entry | `velme-builtins` |
| Goal | task kind (`leaf` / `composite-tail`) and signature `Name(params) -> Output` | HIR |
| Types | every reachable record type with fields | HIR |
| Locals | composite only: each call binding `name: Type = Child(args)` — child **signatures** only, never child IR | HIR (D-5, D-11) |
| Plan | normalized plan text (D-21), fenced and labelled as untrusted user description (§9) | HIR |
| Checks | source text of each check + its lowered form | HIR / `velme-check` |
| Examples | the first `max_prompt_examples` `examples:` items in source order, with literal values (R-SYNTH-38) | HIR (D-7) |
| Budget | effective budget (runtime/30 §7) | HIR + system caps |

**R-SYNTH-08** The prompt contains nothing outside this table: no file paths, no other goals' plans, no environment,
no user identity (tooling/41).
**R-SYNTH-09** Output is constrained to the reply schema (the IR JSON Schema or a question object, R-SYNTH-10) where
the provider supports it; Velme still runs its own full validator on every reply (§20.2) — a provider's schema
guarantee is never trusted.
**R-SYNTH-10** The reply is one IR goal or one question object `{"question": "<text>"}` (R-SYNTH-32). Any prose,
markdown fence, partial JSON or other shape is a failed attempt with `VL0401`.

### 4.1 Token cost (D-44)

The options below live in `[synthesis]` (`tooling/40` §5.1) and apply to the LLM providers only; `external` always
gets the full request (§3.2, R-SYNTH-30). None of them changes which IR is accepted: every reply still passes the full
validator and verification (INV-1, INV-2).

**R-SYNTH-34** The template renders the sections in the order of the §4 table. Header, output contract and allowed
builtins are the same for every goal and attempt with the same options, so they form a fixed prefix. With
`prompt_cache = true` (default) `anthropic` marks the end of that prefix as a cache breakpoint; a prefix below the
model's minimum cacheable length is simply sent uncached. Caching changes price, never the reply, so it does not enter
any key.
**R-SYNTH-35** `schema_in_prompt = "summary"` (default) puts one generated line per IR node kind and the question
object into the output contract and sends the reply schema only through the provider's constraint (R-SYNTH-05,
R-SYNTH-09); `"full"` also puts the whole schema in the prompt, for models that follow it better when they can read it.
**R-SYNTH-36** `reply_format = "ir-json"` (default) asks for canonical IR JSON. `"compact"` asks for the same tree with
every property name and node `kind` tag replaced by a short alias from a fixed table in `velme-synth`, generated
together with the reply schema and versioned with it. `velme-synth` expands a compact reply into canonical IR (a pure
renaming; an unknown alias is `VL0401`) before validation, so the validator, fingerprints, artifacts and lock only ever
see canonical IR (D-21). Retry turns show earlier replies and diagnostic JSON paths in the requested format.
**R-SYNTH-37** `retry_history = "latest"` (default): a retry turn carries only the latest failed reply plus the primary
diagnostic (R-SYNTH-31) of every earlier attempt, so later attempts cost about as much as the second. `"all"` carries
every earlier reply, as R-SYNTH-11 describes. With `stop_on_repeat = true` (default), the loop stops when two attempts
in a row share a cause (R-SYNTH-31), and the goal fails with `VL0403` at once.
**R-SYNTH-38** `max_prompt_examples` (default 8, 0..=64) bounds the examples sent. Every example still runs locally
(§6); a candidate that fails one left out gets it, with its values, in the next retry turn.
**R-SYNTH-39** `retry_model` (optional) is the model used for retries; attempt 0 uses `model`. When set, `model()` is
`<model>+<retry_model>` (Ollama: each with its digest), so the pair enters `synthesis_key` (runtime/32 §2).
**R-SYNTH-40** For LLM providers, `input_version` is `prompt_version` + BLAKE3 of `schema_in_prompt`,
`reply_format`, `retry_history` and `max_prompt_examples`, because they change what is sent. Changing an option only
changes `synthesis_key`; lock staleness uses `contract_key` (D-26), so it never re-synthesizes a locked goal.

## 5. Retry loop (§21)

```
attempt 0 ─► validate (21 §6) ─► verify (§6) ─► accepted
     ▲             │ fail              │ fail
     └── diagnostics appended as a new turn (max_retries = 3)
```

**R-SYNTH-11** On a validation or verification failure, the diagnostics (code, message, JSON path, and for check
failures the input, the assertion and the actual values) are appended as a new turn and the provider is asked again
(which earlier replies the turn carries, and when the loop stops early: R-SYNTH-37).
At most `max_retries` (default and cap: 3) retries follow the first attempt.
**R-SYNTH-12** Transport errors (`RateLimited`, `Unavailable`, `Timeout`) are retried with exponential backoff up to 2
times per attempt and do not consume a synthesis retry.
**R-SYNTH-13** After the last retry the goal fails with `VL0403`, whose message states the cause (R-SYNTH-31);
`--verbose` adds every attempt's diagnostics. The source `plan` is never modified; nothing is written to the artifact
store or the lock.
**R-SYNTH-31** The learner never needs `--verbose` to know what to fix. Each failed attempt has one primary diagnostic
(R-SYNTH-15 for verification, otherwise the first in R-CMP-16 order). Two attempts share a cause when that diagnostic
has the same code and names the same check, example or validator rule, whatever the input. `VL0403` reports the cause
shared by the most attempts, ties going to the later attempt, says how many attempts it covers, and takes its details
(input, values) from the latest attempt with that cause. Each other cause is one note, in first-seen order.

| Cause | Message names | Help suggests |
|---|---|---|
| `VL0502` | the example: given, got, expected | check the example, or say in the plan how that case is handled |
| `VL0501` / `VL0503` | the check and its counterexample (R-SYNTH-19) | say in the plan what happens for that input, or narrow the check with `if … then …` (D-6) |
| `VL0401` / `VL0402` | the validator rule that kept breaking | the plan may need more than the listed builtins can do: simplify it or split the goal |
| `VL0801` | the capability asked for | goals can't use it (INV-4): take the need out of the plan |
| `VL0602` | the operation and input | say in the plan what should happen in that case |
| `VL0601`, `VL0603`..`VL0606` | the limit and the input | make the plan do less work per input, or split the goal |
| `max_calls_per_build` reached (R-SYNTH-21) | the limit and its value | raise it in `[synthesis]`, or build fewer goals at once |
| stopped early: two attempts in a row with the same cause (R-SYNTH-37) | that cause, as in its row above, and "stopped after N attempts" | as that cause's row; `stop_on_repeat = false` retries anyway |

The message and notes quote only what Velme produced: codes, rule names, check and example source, inputs and
computed values. Reply text from the provider never appears in them (R-SYNTH-22).
**R-SYNTH-32** A question reply ends synthesis of that goal at once, with no further retry, and the goal fails with
`VL0407` showing the question. The build never waits for an answer: the learner writes it into the goal, as an
example that shows it or else in the `plan` (language/12 §8.4). Either changes the goal's `contract_key` (runtime/32
§2), so the next build synthesizes it again and the choice stays in reviewed source (INV-3); an example is also
verified. Other goals in the build continue. A question is never written to the store or the lock
and is never cached.
**R-SYNTH-33** The question is untrusted text (R-SYNTH-22). Control characters, newlines and ANSI escapes included,
become spaces and whitespace runs collapse to one; the result must then be 1..=280 Unicode scalar values, or the reply
is a failed attempt with `VL0401`. It is shown only as a note, quoted and labelled as the AI helper's question, and as
a plain string in `--json`.

## 6. Verification pipeline (§22)

A candidate becomes an artifact only after every step passes, in order:

| Step | Check | Failure |
|---|---|---|
| 1–7 | IR validation stages (21 §6): schema, structure, names, types, capabilities, call graph, resources | `VL0401`/`VL0402`/`VL0801` |
| 8a | every `examples:` item runs on the interpreter and matches (D-7) | `VL0502` |
| 8b | every generated input (§7) executes within budget | `VL06xx` |
| 9 | all `check`s hold for every example and generated input | `VL0501` |

**R-SYNTH-14** Verification executes on the reference interpreter with the goal's effective budget; composite goals
run their real, already-accepted children (R-SYNTH-01). A `Timeout` (D-10) is re-run once; a second `Timeout` counts
as a failed attempt but is never recorded as a cached outcome.
**R-SYNTH-15** Any failure in steps 8–9 wraps as `VL0503 VerificationFailed` with the first failing case (examples
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

Synthesis settings live in the `[synthesis]` section of `velme.toml`, defined in `tooling/40` §5.1 (provider,
model, `max_retries` 0..=3 per R-SYNTH-11, timeouts, output tokens, `max_calls_per_build`).

**R-SYNTH-20** API keys come only from the environment (`VELME_API_KEY`, or the `ANTHROPIC_API_KEY` fallback;
`tooling/40` §5.2). A key-like value in `velme.toml` is an error. Keys never appear in logs, traces, fixtures,
artifacts, diagnostics or `--verbose` output; the replay recorder stores no headers.
**R-SYNTH-21** `max_calls_per_build` bounds cost: when reached, remaining goals fail with `VL0403` and a message
naming the limit. The build summary reports calls, tokens in/out, prompt-cache tokens read and written
(R-SYNTH-34) and store/lock hits.

## 9. Untrusted plan text

**R-SYNTH-22** Plan, check and example text are untrusted data: they are fenced in the prompt and the instructions
state they describe intent only. Velme does not rely on this for safety — whatever the LLM returns must still pass
validation and runs sandboxed with no capabilities (INV-1, INV-4, tooling/41).

## 10. Local synthesis log

**R-SYNTH-23** Each attempt appends one JSON line to `.velme/synth-log.jsonl`: time, goal, synthesis key, provider,
model, attempt, outcome code, tokens, latency. Plan text and values are excluded by default. The log never leaves the
machine (tooling/41) and is git-ignored.

## 11. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SYNTH-01 | With a matching lock entry or store hit, a build makes zero provider calls (asserted with a panicking provider). |
| AC-SYNTH-02 | `scripted` provider returning invalid IR twice, with different causes, then valid IR: build succeeds after 2 retries; each retry turn contains the previous diagnostics. |
| AC-SYNTH-03 | Four consecutive invalid replies, no two in a row with the same cause → `VL0403`; no artifact written, lock unchanged, source unchanged. |
| AC-SYNTH-04 | A reply with a `call` node fails validation and the retry turn cites `VL0402` (INV-6). |
| AC-SYNTH-05 | A candidate that passes validation but fails an example is rejected with `VL0502` and never cached. |
| AC-SYNTH-06 | Generated inputs for a fixed goal are byte-identical across two runs and two OSes. |
| AC-SYNTH-07 | The rendered prompt for a golden goal matches its snapshot and contains no child IR, file paths or env values. |
| AC-SYNTH-08 | `--locked` with a stale entry fails with `VL0702`; `--offline` with a cache miss fails with `VL0404`; neither constructs a provider. |
| AC-SYNTH-09 | No API key string appears in any output, log, fixture or artifact after a recorded build (grep test with a sentinel key). |
| AC-SYNTH-10 | `replay` provider reproduces a recorded build byte-for-byte (same artifacts, same lock). |
| AC-SYNTH-11 | Against a mock Ollama server, the `ollama` provider sends the IR JSON Schema as `format` with temperature 0 and no streaming, and an accepted artifact records `"provider": "ollama"` and `<model>@<digest>` as `model_version`. |
| AC-SYNTH-12 | Changing the digest behind the same Ollama tag, with an up-to-date lock, makes zero requests; after deleting the store entry, the next build misses on the new `synthesis_key`. |
| AC-SYNTH-13 | An unreachable Ollama server yields `VL0404`; a model the server doesn't have yields `VL0405` with an `ollama pull` hint. |
| AC-SYNTH-14 | The `external` `synthesize` message for a golden goal matches its snapshot, validates against the committed request schema, and contains no file paths, environment values or API keys. |
| AC-SYNTH-15 | An `external` backend returning hostile IR (a `call` node, an unknown builtin) is rejected with `VL0402` / `VL0801`; nothing is stored. |
| AC-SYNTH-16 | Non-zero exit, timeout, non-JSON stdout, stdout over the cap, and an `{"error"}` reply each fail with `VL0406` naming the goal and backend; the lock is unchanged and no retry is made. |
| AC-SYNTH-17 | With `max_retries = 1`, a second `external` request carries the first reply and its diagnostics in `attempts`; with the default 0, a rejected reply fails the goal after one request. |
| AC-SYNTH-18 | With sentinel values in `VELME_API_KEY` and `ANTHROPIC_API_KEY`, the `external` command's environment contains neither; an `external_command` key in the project `velme.toml` fails with `VL0902`. |
| AC-SYNTH-19 | A fully cached build with `--provider ollama` or `--provider external` sends no request to the server and never starts the command. |
| AC-SYNTH-20 | With `stop_on_repeat = false`, four scripted replies where attempts 1, 2 and 4 fail the same check on different inputs and attempt 3 fails an example: `VL0403` names the check with attempt 4's counterexample and "3 of 4", and one note names the example. A sentinel string in the replies' text appears nowhere in the output. |
| AC-SYNTH-21 | Four scripted replies failing two causes twice each, alternating: `VL0403` reports attempt 4's cause. |
| AC-SYNTH-22 | A scripted `{"question"}` reply with `max_retries = 3` fails the goal with `VL0407` showing the question after exactly one provider call; nothing is stored, the lock is unchanged, and another goal in the same file still builds. |
| AC-SYNTH-23 | A question containing a newline and an ANSI escape is shown with both replaced by spaces; an empty question and a 281-character question are each a failed attempt with `VL0401`. |
| AC-SYNTH-24 | An `external` backend replying `{"question"}` fails the goal with `VL0407`. |
| AC-SYNTH-25 | The rendered prompts for two different golden goals share a byte-identical prefix up to the end of the allowed builtins; the mocked `anthropic` request marks a cache breakpoint there, and with `prompt_cache = false` marks none. Both give the same `synthesis_key`. |
| AC-SYNTH-26 | A scripted `compact` reply expands to IR whose artifact bytes and hash equal those of the same IR sent as `ir-json`; a reply with an unknown alias is a failed attempt with `VL0401`. |
| AC-SYNTH-27 | Three scripted failures with different causes: with `retry_history = "latest"` the third request carries only reply 2 and the primary diagnostics of attempts 1 and 2; with `"all"` it carries replies 1 and 2. |
| AC-SYNTH-28 | Two scripted replies failing the same check on different inputs: `VL0403` after exactly 2 calls, naming the check and "stopped after 2 attempts"; with `stop_on_repeat = false`, 4 calls. |
| AC-SYNTH-29 | With `max_prompt_examples = 2` and 3 examples, the prompt shows the first two; a candidate failing the third is rejected with `VL0502` and the next retry turn shows the third with its values. |
| AC-SYNTH-30 | With `retry_model` set, attempt 0 uses `model` and retries use `retry_model`; the accepted artifact records `<model>+<retry_model>`. Changing `retry_model`, `reply_format` or `max_prompt_examples` changes `synthesis_key`, not `contract_key`, and a build with an up-to-date lock makes zero calls. |
| AC-SYNTH-31 | An `external` backend replying `{"pending": "ticket 42"}` with `max_retries = 3` fails the goal with `VL0408` showing "ticket 42" after exactly one `synthesize` message; nothing is stored, the lock is unchanged, another goal in the same file still builds, and the exit code is 2. When the backend then replies `{"ir"}` to the same request, the next build accepts and locks it. |
| AC-SYNTH-32 | With `max_retries = 1`, a first `external` reply whose IR fails a check and a second reply `{"pending"}` give `VL0408` with a note naming the failed check. An empty pending text gives `VL0406`. |
