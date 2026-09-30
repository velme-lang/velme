# 22 — Spellbook: Synthesis & Verification

**Status:** v0.1 · **Area:** SYNTH
**Read when:** touching the provider interface, a provider (LLM or external backend), the synthesis request, the prompt, the retry loop, the verification pipeline, generated test inputs, or synthesis cost/config.
**Depends on:** [SPEC](../SPEC.md), [20-compiler-architecture](20-compiler-architecture.md), [21-ir](21-ir.md), [runtime/32](../runtime/32-artifacts-cache.md), [tooling/41](../tooling/41-security-privacy.md)

## 1. Purpose & boundaries

Spellbook turns one goal's typed HIR into a **verified** IR artifact. It owns: the provider-neutral interface, the
synthesis request and its external protocol, the prompt contract, structured output, the retry loop, the verification
pipeline and generated test inputs. It does not
own IR validity (that is [21](21-ir.md) §6), execution semantics ([runtime/30](../runtime/30-execution-vibevm.md)) or
the artifact store and lockfile ([runtime/32](../runtime/32-artifacts-cache.md)).

Principle: the provider proposes, the deterministic pipeline disposes (INV-1, INV-2). A provider is an LLM
(`anthropic`, `ollama`) or any service speaking the external protocol (§3.2, D-42); the pipeline treats every
candidate the same way, whoever wrote it.

## 2. When synthesis happens

| Goal kind (20 §3 phase 6) | What is synthesized | LLM call? |
|---|---|---|
| `leaf` | the whole `body` | yes, on cache miss |
| `composite` | the tail `body` over inputs + call bindings (D-5) | yes, on cache miss |
| `wired` (D-4) | nothing — compiler IR | never |

**R-SYNTH-01** Goals are built in post-order of the goal DAG: a composite goal is synthesized only after every child
has an accepted artifact, because verification executes the real children (§6). Synthesis runs one goal at a time;
goals the post-order leaves unordered go in source order (20 §3), so the `max_calls_per_build` cutoff, replay and the
synth log are the same on every run (D-93).
*Future:* goals on the same DAG level synthesized concurrently, each given a call slot by that same order before any
request starts, so the cap cutoff and replay stay deterministic.
**R-SYNTH-02** Lookup order before any provider call: (1) `velme.lock` entry whose `contract_key` matches (runtime/32 §5), (2) artifact
store by `synthesis_key` (D-11, runtime/32 §2), after the identity step (R-SYNTH-25), (3) provider. A hit at (1)
makes no provider contact; a hit at (2) makes no provider call (R-SYNTH-21).
`velme-runtime` performs this lookup and calls into `velme-synth` only for goals that reach (3) (D-54); `velme-synth`
itself owns nothing about the lock or store.
**R-SYNTH-03** `--locked` and `--offline` never construct a provider (20 R-CMP-19). A goal that would need synthesis
fails with `VL0702 LockStale` (`--locked`) or `VL0404 ProviderUnavailable` (`--offline`).

## 3. Provider interface (D-13, D-14, D-41, D-42, INV-7)

### 3.1 Trait and providers

```rust
#[async_trait]
pub trait SynthProvider: Send + Sync {
    fn id(&self) -> &str;                    // "anthropic" | "ollama" | "external" | "replay" | "scripted"
    fn model(&self) -> &str;                 // model id (or model+retry_model, R-SYNTH-39), Ollama digest or external <backend>@<backend_version> → manifest model_version
    fn input_version(&self) -> &str;         // prompt_version + request options (LLM providers, R-SYNTH-40) or request_version (external)
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits)
        -> Result<SynthReply, ProviderError>;
}

pub struct SynthRequest { pub request_version: String, pub ir_version: String, pub builtins_version: String,
                          pub task: TaskKind, pub goal: String,
                          pub signature: Signature, pub types: Vec<RecordType>, pub locals: Vec<LocalBinding>,
                          pub plan: String, pub checks: Vec<CheckItem>, pub examples: Vec<Example>,
                          pub budget: Budget, pub builtins: Vec<BuiltinSig>,
                          pub attempts: Vec<AttemptFeedback>,                // earlier replies + diagnostics (§5)
                          pub output_schema: serde_json::Value }             // reply schema: goal body or question (R-SYNTH-10)
pub struct SynthReply  { pub reply_json: String, pub usage: Usage, pub latency: Duration }
pub enum  ProviderError { NotConfigured, KeyRejected, TokenRejected, Unavailable(String), RateLimited { retry_after: Option<Duration> },
                          Refused(String), Timeout, Malformed(String),
                          BackendFailed { reason: String, body: String },        // R-SYNTH-28: body = cleaned last 4 KiB of the reply
                          Pending(String), File { path: String, reason: String }, // R-SYNTH-43
                          Internal(String) }
```

`SynthRequest` is the structured form of the §4 table. LLM providers render it into a prompt with the versioned
template (§4); the `external` provider sends it as JSON (§3.2); `replay` and `scripted` ignore it except for keying.
`ir_version` and `builtins_version` are the versions the candidate must be written against (D-97).

**R-SYNTH-04** The trait and its types are plain Rust + `serde_json`. Vendor SDK/HTTP types never appear in a
signature; each provider is a module in `velme-synth` behind a Cargo feature (`provider-anthropic`).
**R-SYNTH-05** Providers:

| Provider | Use | Behaviour |
|---|---|---|
| `anthropic` | hosted LLM (D-14) | Messages API, temperature 0; output through two tools, `write_goal` (input: `{"body": <expression>}`, D-103) and `ask_question` (input: the question object), with the model forced to call one (R-SYNTH-44) |
| `ollama` | local LLM (D-41) | Ollama chat API at the configured URL, `format` set to the reply JSON Schema (R-SYNTH-10), temperature 0, no streaming; no API key |
| `external` | human- or tool-written goal bodies (D-42) | speaks the §3.2 protocol over HTTP and JSON to a standalone service the user starts, at the URL the user configures (D-101); Velme starts no process |
| `replay` | integration tests, golden builds, CI | reads `<replay_dir>/b3-<hex>.json`, named from the synthesis key as store files are (R-SYNTH-43); missing fixture → `VL0404` naming the key; `VELME_SYNTH_RECORD=1` with a live provider writes fixtures |
| `scripted` | unit tests of the retry loop and pipeline | a queue of replies/errors: in memory from library tests, or from the script file in `VELME_SYNTH_SCRIPT` in a `velme-cli` built with the `test-provider` feature, never in release builds (`tooling/40` §5.2) |

**R-SYNTH-06** Live-provider tests run only with `VELME_LIVE_LLM=1` and are never part of the default gate (D-13).
**R-SYNTH-43** Replay fixtures (D-94). `<replay_dir>/replay.json` holds `provider`, `model_version` and
`input_version` of the recorded build, and for `external` its `backend` name (`runtime/32` R-ART-21), and, when they
aren't the defaults, its `retry_history` and `reply_format` (R-SYNTH-36, R-SYNTH-37), which change the bytes of the requests the fixtures are
keyed on and how the recorded replies are read; `replay` reports them from the identity step (R-SYNTH-25), and a build
replaying uses the recorded `retry_history` and `reply_format` over its own, so the replayed
build computes the same keys and writes byte-identical manifests. A replay reproduces the artifacts, lock, manifests
and diagnostic codes of the recorded build; the detail and notes of a provider failure may be less specific than the
recorded run's, since fixtures keep only the error variant and no provider prose. `replay.json` and the fixtures are read
bounded and never through a link (`runtime/32` R-ART-10), and written whole through a temporary file, refusing a link
or a non-regular file in the way, with `VL0901` (R-ART-09). `b3-<hex>.json`, named from the synthesis key as store files are, holds one entry per exchange
of that goal, in order: `request`, the BLAKE3 of the canonical JSON (21 R-IR-21) of the `SynthRequest` sent; then
either `reply`, the reply JSON, or `error`, the `ProviderError` variant (`refused`, `malformed`, `backend_failed`,
`pending`; `pending` with its R-SYNTH-41-cleaned text); and `usage`. No prompt body, header or key is stored, so plan
text never lands in a fixture. On replay, an entry whose `request` differs from the hash of the request being sent,
or a request after the last entry, is `VL0404` naming the key and the attempt, with the help "re-record the fixture
with `VELME_SYNTH_RECORD=1`".
The recorder (`VELME_SYNTH_RECORD=1` with a live provider) writes every exchange that reached the provider's reply —
rejected candidates, questions, pending and refused replies included; transport retries are not exchanges — as
canonical JSON, overwrites the goal's whole fixture file each time it synthesizes that goal, and writes `replay.json`.
**R-SYNTH-44** `anthropic` sets `tool_choice` so the model must call `write_goal` or `ask_question`; the tool's input
is the reply (R-SYNTH-10), and a reply with no tool call, both tools or anything else is `Malformed`. Tests reach a
mock server through a base URL set only by the test-only constructor in `velme-test-support`, never by a config key
or environment variable (D-98).
**R-SYNTH-07** `ProviderError` mapping: `NotConfigured` → `VL0405`; `KeyRejected` (the API answered 401 or 403 to a key
that is set) → `VL0405` worded "the API key was rejected" (`reference/90`); `TokenRejected` (an `external` service
answered 401 or 403) → `VL0405` worded "the external backend rejected the token", with a hint naming `VELME_EXTERNAL_TOKEN`; `File` (a replay file that is a link, is
not a regular file, is too large or can't be read or written) → `VL0901`; `Unavailable`/`Timeout`/`RateLimited` after
transport retries → `VL0404`; `Refused`/`Malformed` count as a failed attempt (§5) with `VL0401`, in Velme's own
wording, never the provider's text (R-SYNTH-22, D-93); `BackendFailed` → `VL0406`, not retried; `Pending` → `VL0408`,
not retried (R-SYNTH-41); `Internal` (a Velme bug, such as a request with no hash) → `VL0607`, not retried.
**R-SYNTH-45** After one goal ends with `VL0404`, or with `VL0405` from a call (a rejected key or a model the API
doesn't know) or `VL0901` from a replay file, the build contacts the provider no more: every later goal that reaches
R-SYNTH-02 step 3 ends with the same code without a request (D-93). A build whose store answers a goal needs no key:
the Anthropic provider is built without one, its identity step contacts nothing, and only a request ends with `VL0405`.
**R-SYNTH-24** `ollama` resolves the configured model's digest from the server's `/api/tags` **on the first lock miss in
a build**, not unconditionally, and reports `<model>@<digest>` as `model()`, with a model name that has no tag
normalized to `<name>:latest` and the digest kept whole, `sha256:` prefix included (D-98); the result is reused for
every store lookup and request in that build (D-57). A tag that now points at different weights therefore changes
`synthesis_key` but never `contract_key` (runtime/32 R-ART-03). A model missing on the server is `NotConfigured`
(`VL0405`, help: `ollama pull <model>`); an unreachable server is `Unavailable` (`VL0404`).
**R-SYNTH-25** The **identity step** is separate from constructing the provider (D-92). It yields the provider id,
`model()` and `input_version()` that `synthesis_key` needs, and runs once, on the first lock miss in a build (R-SYNTH-02
step 1), before the store lookup: `ollama` resolves its digest (R-SYNTH-24), `external` sends `describe`
(R-SYNTH-26), `replay` reads `replay.json` (R-SYNTH-43), and `anthropic` and `scripted` contact nothing. The provider
that makes requests is constructed only when a goal also misses the store (step 3). An `ollama` server or an
`external` service is therefore never contacted for a fully cached build — one where every goal is fresh in the lock
(D-57).

### 3.2 External backend protocol (D-42, D-101)

The `external` provider lets a person or any tool supply the implementation instead of an LLM. The backend is a
standalone service that the user starts; Velme starts no process. Velme sends HTTP requests with JSON bodies to the
service's base URL (`tooling/40` R-CLI-13) and reads one JSON document from each response. The request body follows the
committed `velme-synth-request` JSON Schema, generated from the Rust types like the IR schema (21 R-IR-20).

| Endpoint | Velme sends | Backend replies |
|---|---|---|
| `GET /v1/describe` | no body | `{"backend": "<name>", "backend_version": "<version>"}` |
| `POST /v1/synthesize` | the canonical `SynthRequest` as the body (`Content-Type: application/json`) | `{"body": <expression>}`, `{"question": "<text>"}`, `{"pending": "<text>"}` or `{"error": "<reason>"}` |

The endpoints are relative to the base URL, so a service may live under a path prefix. A service may add fields to its
replies (unknown keys are ignored, as in `describe`); a change that isn't compatible with this protocol goes to `/v2`. `SynthRequest` carries its own
`request_version` (`"0.1"`); the `/v1/` prefix versions the endpoints.

**R-SYNTH-26** `describe` runs once, on the first lock miss in a build (D-57), and is skipped entirely for a fully
cached build; its `model()` is `<backend>@<backend_version>` (as Ollama's is `<model>@<digest>`) and enters `synthesis_key`, so a new
backend or version is a cache miss but never makes a lock stale (runtime/32 R-ART-03). `backend` and `backend_version` must each be 1..=128 Unicode scalar
values after whitespace runs collapse to one space, and must hold no control character, Unicode format (`Cf`) character
or bidi control (D-47): those are refused, not cleaned away. Any other key of the reply is ignored, so services may add fields; a `describe` that
fails in any R-SYNTH-28 way or breaks this is `VL0406` (D-98).
**R-SYNTH-27** A `synthesize` reply's `body` is handled exactly like an LLM reply: Velme completes it into an IR goal
(R-SYNTH-10), then full validation (21 §6) and verification (§6). The backend gets no trust the LLM doesn't get (INV-1). A `question` reply follows
R-SYNTH-32..33, as an LLM's does.
**R-SYNTH-28** Transport uses the same HTTP client and transport retries as `ollama` (R-SYNTH-12, D-101). A refused
connection, a DNS failure or a timeout is `Unavailable` or `Timeout` → `VL0404` after those retries, and the build then
contacts the provider no more (R-SYNTH-45); so does a body that stalls after its headers. A `429` is `RateLimited` → `VL0404` and a `408` is retried like a `5xx` (then `VL0406` if it persists). A `401` or `403` is `TokenRejected` → `VL0405` (R-SYNTH-07), not retried: "rejected the token" when a token was sent, "wants a token" (hint: set `VELME_EXTERNAL_TOKEN`) when none was.
A `5xx` after the retries, any other non-2xx status (a redirect included: redirects are never followed), a body above
2 MiB, a body that is not one JSON document, a reply that does not hold exactly one of the four kinds above (any other key is ignored), a reply whose `body` holds the
bearer token ("the reply contains your token"), or an `{"error"}` reply is `BackendFailed` → `VL0406` naming the goal and backend, with the last 4 KiB of the response body in the notes,
cleaned and escaped like any untrusted text (T-13). Proxies and TLS: for a host that isn't loopback, the client honours
`ALL_PROXY`, `HTTPS_PROXY` and `HTTP_PROXY` from the environment through a CONNECT tunnel; a loopback host is never
proxied. TLS trusts only the bundled `webpki-roots`, so a TLS-inspecting proxy or a private CA fails with `VL0404`; a
user-level `external_ca_file` setting is planned for the M6 config work (D-102). `external_timeout_secs` bounds each request, `describe` included; until the user-level config arrives with M6 it is fixed at its default of 30 s.
Nothing is written to the store or the lock (D-98).
**R-SYNTH-29** The base URL is a `https` URL, or a plain `http` URL whose host is `localhost`, in 127.0.0.0/8, or `[::1]`; every other host needs `https`. `localhost`
connects to 127.0.0.1 or `::1` without asking DNS, so a resolver can't send it elsewhere. The URL's path holds only RFC 3986
`pchar`s and `/`. Any other scheme, user information (`user:pass@`) in the URL, or a URL that
does not parse is `VL0902` before any contact. The URL comes only from the `--external-url` flag, `VELME_EXTERNAL_URL` or
the user-level config, never from the project's `velme.toml` (`tooling/40` R-CLI-13, `tooling/41` T-10). An optional
bearer token from `VELME_EXTERNAL_TOKEN` is sent as `Authorization: Bearer <token>` to that URL and nowhere else, and is
never logged, echoed or recorded (`tooling/41` R-SEC-13); a token that is shorter than 16 characters or contains
whitespace, a control character or a non-ASCII character is an error (`VL0405`, unusable token), not left out (leading
and trailing whitespace is trimmed). The token is taken out of the message texts the service sends (the body before the
4 KiB tail is cut, `error`, `question` and `pending` texts, the `describe` name and version) before they are kept or shown;
a `body` is never rewritten, and one that holds the token, plain or JSON-escaped, is refused (D-102). The service runs outside Velme's sandbox and with whatever authority its owner gave it.
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

**R-SYNTH-42** When a goal ends with no artifact, for any reason (`VL0403`..`VL0409`, a `VL06xx` such as `VL0603`
from R-SYNTH-14, or `VL0607`), none of its ancestors is synthesized (D-56, D-93): each ancestor ends instead with `VL0409 SynthesisBlocked`, "`{goal}` wasn't built
because `{child}` {reason}.", with the child's failure code as a note. `VL04xx` exits with status 2 as before
(`tooling/40`).

## 4. Prompt contract

The prompt is rendered from `SynthRequest` (§3) with a versioned template in `crates/velme-synth/prompts/`, one file
per task kind (`leaf`, `composite`), each holding its retry-turn text too. `prompt_version` is one value covering every template: their
ids + BLAKE3 of all the template bytes (an edit to one re-keys every goal), the compact alias table (R-SYNTH-36) and the schema summary lines (R-SYNTH-35), so
an edit to any of them, or to an option that shapes the prompt (R-SYNTH-40), changes synthesis keys (D-11, D-97). It does
not cover the code that renders a template or the providers' tool descriptions: like any compiler code change, an edit
there re-keys nothing (`runtime/32` R-ART-04). Both LLM providers share the templates. The table below is also the content of `SynthRequest`; the external protocol (§3.2)
sends the same fields as structured JSON.

| Section | Contents | From |
|---|---|---|
| Header | `prompt_version`, `ir_version`, `builtins_version` | constants |
| Output contract | "return `{"body": <expression>}`, the goal's body as one IR expression node, which is all Velme asks for (D-103); use only listed builtins; only if the plan leaves open a choice that changes the result, return `{"question": …}` instead" + the reply schema or its summary (R-SYNTH-35) | `velme-ir` / `velme-synth` |
| Rules and worked example | in the reply's format (the `compact` aliases when asked, R-SYNTH-36): one line each: an input is read with an `input` node, a record's field with a `field` node and never a dotted name, `local` only for a lambda's parameter or a call result; checks and examples describe the result and are never the body, and `result` is not a name the body can read. Arithmetic and comparison are `binary` ops and never builtins, and the `unary` ops are listed too (both from the IR schema). Then three fixed worked examples, for a made-up goal that no example of the repository uses, each a valid body and together showing a `builtin` call, a top-level `binary` node and a lambda's `local` (D-104) | template |
| Allowed builtins | name + signature of each catalog entry | `velme-builtins` |
| Goal | task kind (`leaf` / `composite-tail`) and signature `Name(params) -> Output` | HIR |
| Types | every reachable record type with fields | HIR |
| Locals | composite only: each call binding `name: Type = Child(args)` — child **signatures** only, never child IR | HIR (D-5, D-11) |
| Plan | normalized plan text (D-21), fenced and labelled as untrusted user description (§9) | HIR |
| Checks | the source text of each check, and nothing else: its lowered form reads like a body and models copy it (D-104). The `SynthRequest` still carries the lowered form for `external` (§3.2) | HIR |
| Examples | the first `max_prompt_examples` `examples:` items in source order, with literal values (R-SYNTH-38) | HIR (D-7) |
| Budget | effective budget (runtime/30 §7) | HIR + system caps |

**R-SYNTH-08** The prompt contains nothing outside this table: no file paths, no other goals' plans, no environment,
no user identity (tooling/41).
**R-SYNTH-09** Output is constrained to the reply schema (a `{"body"}` object over the IR expression schema, or a question object, R-SYNTH-10) where
the provider supports it; Velme still runs its own full validator on every reply — a provider's schema
guarantee is never trusted.
**R-SYNTH-10** The reply is one object `{"body": <expression>}`, `body` being one IR expression node (`compiler/21` §3), or one
question object `{"question": "<text>"}` (R-SYNTH-32). Only `body` is read from it, and Velme writes the rest of the
IR goal itself from the request: `ir_version` and `builtins_version` (§3.1), `goal`, `inputs` and `output` from the
signature, and `types` from the request's `types`, which are exactly the record types R-IR-01 reaches (`compiler/21`
D-84); `calls` is left out and the compiler's are joined in (R-IR-02). The assembled goal then passes the full validator
(`compiler/21` §6), so every rule that judges a body still judges it, and no reply can differ from its goal's signature
(D-103). Any prose, markdown fence, partial JSON, a reply with no `body`, one holding both a `body` and a `question`
(neither wins) or another shape is a failed attempt with `VL0401`, as is a number in the `body` that canonical JSON can't hold; a `body` that is not an expression, or repeats a key, fails at validation stage 1, at a JSON path under `/body`. The reply schema and
the summary of R-SYNTH-35 leave out the `call` node, which only the compiler's `calls` may hold (a body reads a call's
result as a `local`).

### 4.1 Token cost (D-44)

The options below live in `[synthesis]` (`tooling/40` §5.1) and apply to the LLM providers only; `external` always
gets the full request (§3.2, R-SYNTH-30). None of them changes which IR is accepted: every reply still passes the full
validator and verification (INV-1, INV-2).

**R-SYNTH-34** The template renders the sections in the order of the §4 table. Header, output contract and allowed
builtins are the same for every goal and attempt with the same options, so they form a fixed prefix. With
`prompt_cache = true` (default) `anthropic` marks the end of that prefix as a cache breakpoint; a prefix below the
model's minimum cacheable length is simply sent uncached. Caching changes price, never the reply, so it does not enter
any key.
**R-SYNTH-35** `schema_in_prompt = "summary"` (default) puts one generated line per IR expression node kind and the question
object into the output contract and sends the reply schema only through the provider's constraint (R-SYNTH-05,
R-SYNTH-09); `"full"` also puts the whole schema in the prompt, for models that follow it better when they can read it.
**R-SYNTH-36** `reply_format = "ir-json"` (default) asks for canonical IR JSON, `{"body": …}`. `"compact"` asks for the same tree with
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

## 5. Retry loop

```
attempt 0 ─► validate (21 §6) ─► verify (§6) ─► accepted
     ▲             │ fail              │ fail
     └── diagnostics appended as a new turn (max_retries = 3)
```

**R-SYNTH-11** On a validation or verification failure, the diagnostics (code, message, JSON path, and for check
failures the input, the assertion and the actual values) are appended as a new turn and the provider is asked again
(which earlier replies the turn carries, and when the loop stops early: R-SYNTH-37). For LLM providers the turns are
real conversation turns: each carried reply as an assistant turn, then one user turn with the diagnostics, rendered
by the template's retry text (D-95).
At most `max_retries` (default and cap: 3) retries follow the first attempt.
**R-SYNTH-12** Transport errors (`RateLimited`, `Unavailable`, `Timeout`) are retried up to 2 times per attempt and
do not consume a synthesis retry. The waits are 1 s then 2 s with no jitter; a `RateLimited` with `retry_after` waits
that long instead, at most 30 s. The sleep is injected, so tests take no wall time (D-95).
**R-SYNTH-13** After the last retry the goal fails with `VL0403`, whose message states the cause (R-SYNTH-31);
`--verbose` adds every attempt's diagnostics. The source `plan` is never modified; nothing is written to the artifact
store or the lock.
**R-SYNTH-31** The learner never needs `--verbose` to know what to fix. Each failed attempt has one primary diagnostic
(R-SYNTH-15 for verification, otherwise the first in R-CMP-16 order). Two attempts share a cause when that diagnostic
has the same code and names the same check, example or validator rule (same stage and rule name, whatever the JSON
path, D-95), whatever the input. `VL0403` reports the cause
shared by the most attempts, ties going to the later attempt, says how many attempts it covers, and takes its details
(input, values) from the latest attempt with that cause. Each other cause is one note, in first-seen order.

| Cause | Message names | Help suggests |
|---|---|---|
| `VL0502` | the example: given, got, expected | check the example, or say in the plan how that case is handled |
| `VL0501` / `VL0503` | the check and its counterexample (R-SYNTH-19) | say in the plan what happens for that input, or narrow the check with `if … then …` (D-6) |
| `VL0401` / `VL0402`, a built-in that doesn't exist or is called wrongly (`names-11`, `types-6`, `types-7`, `types-25`, `types-26`) | the validator rule that kept breaking | the plan may need more than the listed builtins can do: simplify it or split the goal |
| `VL0401` / `VL0402`, a body too large (`structure-1`, `structure-3`, `resources-*`) | the validator rule that kept breaking | the plan may be too big for one goal: simplify it or split the goal (D-103) |
| `VL0401` / `VL0402`, a reply that isn't a body (`schema-1`, or no `body`, or not JSON) | what was wrong with the reply | the AI helper's reply wasn't in a form Velme can use: build again, or try another model (D-103) |
| `VL0401` / `VL0402`, any other rule | the validator rule that kept breaking | the AI helper's code didn't fit the goal: build again, or add an example that shows the result (D-103) |
| `VL0801` | the capability asked for | goals can't use it (INV-4): take the need out of the plan |
| `VL0602` | the operation and input | say in the plan what should happen in that case |
| `VL0601`, `VL0604`..`VL0606` | the limit and the input | make the plan do less work per input, or split the goal |
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

**R-SYNTH-49** The diagnostic a retry carries for a name or field the goal doesn't have says what is there and which
node reads it, from the request alone, with the node named as the reply is asked to spell it (the alias too in the
`compact` format, R-SYNTH-36). For `names-7` and `names-8` it lists the goal's inputs with their types (each read with
an `input` node) and, for a composite, the call bindings (each read with a `local` node); for a dotted name it says that
a field is read with a `field` node over the `input` node, or over the `local` node when the name starts at a call
binding; for `names-9` and `names-10` it lists the fields of the record named; for `types-15` on `add`, `sub`, `mul` or
`div` with a Text operand, that these are for Numbers and Text is joined with `concat`, and for no other operator or
operand, nothing; for a `names-11` name that is an operator of the schema, that it is written as a `binary` (or `unary`)
node with that `op`, and for any other name, the built-ins that the request's catalog lets a `builtin` node
call (not the collection primitives). The validator reports what a finding is about as data, not text, so the hint never
depends on parsing its message. Each distinct hint is sent once per attempt, on the first finding that has it. Nothing
of the reply's own text is repeated in a hint (R-SYNTH-22) (D-104).

## 6. Verification pipeline

A candidate becomes an artifact only after every step passes, in order:

| Step | Check | Failure |
|---|---|---|
| 1–7 | IR validation stages (21 §6): schema, structure, names, types, capabilities, call graph, resources | `VL0401`/`VL0402`/`VL0801` |
| 8a | every `examples:` item runs on the interpreter and matches (D-7) | `VL0502` |
| 8b | every generated input (§7) executes within budget | `VL06xx` |
| 9 | all `check`s hold for every example and generated input | `VL0501` |

**R-SYNTH-14** Verification executes on the reference interpreter with the goal's effective budget, through the
`ChildRunner` trait `velme-runtime` implements and passes in, so `velme-synth` never depends on the artifact store
directly (D-54, `compiler/20` R-CMP-20). The runner takes the whole candidate goal and one input and runs it through
`velme-runtime`'s own scheduler, a composite's real, already-accepted children (R-SYNTH-01) included; `velme-synth`
never calls back once per child, so wave order and argument evaluation (`runtime/30` R-RUN-17, D-91) exist in one
place (D-98). A `VL0603` from the watchdog is never a verdict on the candidate (D-51): the goal ends at once with
`VL0603` (exit 3), nothing is stored, no further call is made for it, and its ancestors get `VL0409` (R-SYNTH-42,
D-93).
**R-SYNTH-46** When re-verification after a child changed (runtime/32 R-ART-22) fails, the ancestor's first request
carries the previous artifact's IR and that failure's diagnostics as an earlier attempt in `attempts`, as a retry turn
would; this uses up no retry. The learner sees the failure as a note naming the child that changed (D-95).
**R-SYNTH-47** A candidate that passes verification but that the store then refuses (runtime/32 R-ART-10, R-ART-11) is
`VL0607`, a Velme bug, not a failed attempt (D-93).
**R-SYNTH-15** Any failure in steps 8–9 but `VL0603` (R-SYNTH-14) wraps as `VL0503 VerificationFailed` with the first
failing case (examples before generated inputs, then generation order) as the primary cause.
**R-SYNTH-16** Only a fully verified candidate is eligible for the artifact store (runtime/32 R-ART-11).

## 7. Generated test inputs (D-96)

The `check` block is a property, not a proof: it is evaluated over a bounded, deterministic input set. An **input** is
one value per parameter; its identity and size are those of the canonical JSON (21 R-IR-21) of the object mapping each
parameter name to its value (language/11 §10).

| Stage | Inputs |
|---|---|
| 1. Examples | the goal's `examples:` inputs, in source order |
| 2. Boundary | the diagonal of the parameters' boundary sets (§7.1) |
| 3. Pairwise | the pairwise rows (§7.2) over the parameters' small sets |
| 4. Random | per-input SplitMix64 streams seeded from `contract_key` (§7.3) |

**R-SYNTH-17** Input generation is a pure function of the goal's `contract_key` (runtime/32 §2), so changing model or prompt does not change it: the same goal always gets the
same inputs on every machine (INV-3).
**R-SYNTH-48** The generator — value sets, diagonal, pairwise algorithm, seed, streams, distributions, alphabet, slot
split and caps, as this section defines them — is frozen per `language_version`. Any change to it is a breaking change
that ships only with a new `language_version`, which enters `contract_key` (runtime/32 R-ART-23), so `test --locked`
never re-verifies a lock against inputs it wasn't verified with (D-96).

### 7.1 Value sets

Each set is an ordered list. `b`ᵢ is `B(T)[i mod |B(T)|]` and `s`ᵢ is `S(T)[i mod |S(T)|]` for a list's item type `T`.

| Type | Boundary set `B` | Small set `S` |
|---|---|---|
| `Number` | `0, 1, -1, 0.5, 1000000, -1000000` | `0, 1, -1` |
| `Boolean` | `true, false` | `true, false` |
| `Text` | `"", "a", "Lina", "é🙂"` (U+00E9, U+1F642) | `"", "a"` |
| `T?` | `nothing`, then `B(T)` | `nothing`, then `S(T)` |
| `List<T>` | `[]`, `[b₀]`, `[b₀, b₁, b₂]`, `[b₀, b₀]` | `[]`, `[s₀]`, `[s₀, s₁]` |
| record | the diagonal of its fields' `B` sets | the diagonal of its fields' `S` sets |

The **diagonal** of ordered sets X₀..X₍ₖ₋₁₎ (fields or parameters, in declaration order) has `m = max |Xⱼ|` rows;
row `i` (0 ≤ i < m) takes `Xⱼ[i mod |Xⱼ|]` for each `j`. With k = 0 it is one empty row. Nested types apply the table
recursively.

### 7.2 Pairwise

Over the parameters p₀..p₍ₖ₋₁₎ with `nⱼ = |S(Tⱼ)|`, the rows are index tuples, each mapped to the input taking
`S(Tⱼ)[row[j]]` for pⱼ:

- k = 0: no rows. k = 1: `(0)`, `(1)`, …, `(n₀ − 1)`.
- k ≥ 2: `U` = every pair `(j, a, l, b)` with `j < l`, `a < nⱼ`, `b < nₗ`. While `U` is not empty: take its least
  pair in lexicographic order of `(j, a, l, b)` and set `row[j] = a`, `row[l] = b`; then for every other position `r`,
  ascending, set `row[r]` to the `v < nᵣ` that maximizes the number of already-set positions `f` whose pair of
  `(f, row[f])` and `(r, v)`, written lower position first, is still in `U`, taking the least `v` on a tie. Emit the
  row, then remove from `U` every pair it covers.

### 7.3 Random

`seed` is the first 8 of the 32 digest bytes that `contract_key`'s hex spells, read as a little-endian `u64`. The
SplitMix64 stream with seed `s` has as its `j`-th output (`j` ≥ 1) `mix(s + j · 0x9E3779B97F4A7C15)`, all arithmetic
wrapping mod 2⁶⁴, where `mix(z)`: `z = (z ^ (z >> 30)) · 0xBF58476D1CE4E5B9`; `z = (z ^ (z >> 27)) ·
0x94D049BB133111EB`; `z ^ (z >> 31)` — the stream of language/14 R-BLT-04 before its `[0, 1)` mapping. Random attempt
`i` (0-based) draws from its own stream, seeded with the `(i + 1)`-th output of the stream seeded with `seed`.
`below(n)` takes the stream's next output `x` and is `⌊x · n / 2⁶⁴⌋` (a 128-bit product, no rejection).

A value is drawn depth first: parameters and record fields in declaration order, list items in order.

| Type | Draw |
|---|---|
| `Number` | if `below(2) = 0`: `below(2001) − 1000`; else `(below(200001) − 100000) / 100` |
| `Boolean` | `below(2) = 1` |
| `Text` | length `below(17)`, then each scalar `A[below(32)]`, `A` = `a`..`z`, `A`, `Z`, `0`, space, `é` (U+00E9), `🙂` (U+1F642), in that order |
| `T?` | `nothing` if `below(4) = 0`, else a `T` |
| `List<T>` | length `below(9)`, then the items |
| record | its fields |

### 7.4 Slots, order and de-duplication

**R-SYNTH-18** At most 64 inputs per goal. All `E` examples come first, in source order, and are never dropped; the
`G = max(0, 64 − E)` generated slots are filled in stage order: boundary candidates until `min(G, 16)` generated inputs
are accepted, then pairwise rows until `min(G, 40)` are, then random attempts until `G` are or 256 attempts have been
made. A stage's unused slots pass to the next. A candidate is accepted unless its canonical JSON equals an earlier
input's (examples included), is over 256 KiB (262 144 bytes), or would be refused by input decoding (language/11
R-TYP-24, `MAX_DEPTH`); a refused candidate takes no slot but still uses its random attempt. The accepted order is the
generation order of R-SYNTH-15. Since each random attempt has its own stream, an implementation may stop building a
candidate once it is known to be over 256 KiB.

*Golden vectors* (canonical JSON, generation order; a conforming generator reproduces them exactly):

```
V1  Double(n: Number) -> Number, no examples
    contract_key b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262 (seed 0xa6a1f9f5b94913af)
    6 boundary, 0 pairwise (all duplicates), 58 random
    {"n":0} {"n":1} {"n":-1} {"n":0.5} {"n":1000000} {"n":-1000000}
    {"n":-502.7} {"n":-756} {"n":-710} {"n":95.63} {"n":-739} {"n":-343} {"n":-908.11}

V2  Greet(name: Text, loud: Boolean) -> Text, one example (name "Lina", loud false)
    contract_key b3:0000000000000000000000000000000000000000000000000000000000000000 (seed 0)
    1 example, 4 boundary, 2 pairwise, 57 random
    {"loud":false,"name":"Lina"}
    {"loud":true,"name":""} {"loud":false,"name":"a"} {"loud":true,"name":"Lina"} {"loud":false,"name":"é🙂"}
    {"loud":false,"name":""} {"loud":true,"name":"a"}
    {"loud":false,"name":"wmvzeyimsmd"} {"loud":true,"name":"g0en"} {"loud":false,"name":"ArobAoxfrwvpdzh"}
    {"loud":true,"name":"ur0e🙂"}

V3  type Item: name: Text, price: Number
    Total(items: List<Item>, bonus: Number?) -> Number, no examples
    contract_key b3:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef (seed 0xefcdab8967452301)
    7 boundary, 10 pairwise, 47 random
    {"bonus":null,"items":[]}
    {"bonus":0,"items":[{"name":"","price":0}]}
    {"bonus":1,"items":[{"name":"","price":0},{"name":"a","price":1},{"name":"Lina","price":-1}]}
    {"bonus":-1,"items":[{"name":"","price":0},{"name":"","price":0}]}
    … 3 more boundary, then pairwise:
    {"bonus":0,"items":[]} {"bonus":1,"items":[]} {"bonus":-1,"items":[]}
    {"bonus":null,"items":[{"name":"","price":0}]}
    … 6 more pairwise, then random:
    {"bonus":878,"items":[{"name":"b","price":985.59},{"name":"ézqdojntxafulf","price":482},{"name":"ZAe ","price":172},{"name":"lymgcelohwqbgv","price":-898}]}
```

**R-SYNTH-19** A check failure on a generated input is shown to the learner as a counterexample ("your check fails
when `players` is `[]`"), so they can refine the check; the message suggests the `if … then …` rewrite
(`if price >= 0 then result >= 0`, D-6). An `assume:` precondition block is Future; the word is reserved (D-39).

## 8. Configuration & secrets

Synthesis settings live in the `[synthesis]` section of `velme.toml`, defined in `tooling/40` §5.1 (provider,
model, `max_retries` 0..=3 per R-SYNTH-11, timeouts, output tokens, `max_calls_per_build`).

**R-SYNTH-20** API keys come only from the environment (`VELME_API_KEY`, or the `ANTHROPIC_API_KEY` fallback;
`tooling/40` §5.2). A key-like value in `velme.toml` is an error. Keys never appear in logs, traces, fixtures,
artifacts, diagnostics or `--verbose` output; the replay recorder stores no headers.
**R-SYNTH-21** `max_calls_per_build` bounds cost: when reached, remaining goals whose children all built fail with
`VL0403` and a message naming the limit; the rest get `VL0409` (R-SYNTH-42). A **provider call** is one attempt at a
candidate, whatever its reply, a `{"pending"}` or `{"question"}` included (D-56). Transport retries (R-SYNTH-12), the
Ollama digest lookup and `describe` are not calls and never count toward the cap, but they are provider contact: a
build that must make none, such as a fully cached one (D-57), makes none of them either (D-92). Each `complete()`
invocation is exactly one call, even when every transport try inside it failed and it ended in `VL0404`. The build summary reports calls, tokens in/out, prompt-cache tokens read and written
(R-SYNTH-34) and store/lock hits. A lock hit is verified again on every build, leaves included, on the interpreter with
no provider call; one that no longer passes is stale and synthesized in the ordinary way (runtime/32 R-ART-22, D-100).

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
| AC-SYNTH-03 | Four consecutive invalid replies, no two in a row with the same cause → `VL0403`; no artifact written, no new lock entry, source unchanged; a locked artifact that failed its examples or checks on re-verification, with no replacement built, loses its entry; any other goal keeps its old one (runtime/32 R-ART-22, D-100). |
| AC-SYNTH-04 | A reply with a `call` node fails validation and the retry turn cites `VL0402` (INV-6). |
| AC-SYNTH-05 | A candidate that passes validation but fails an example is rejected with `VL0502` and never cached. |
| AC-SYNTH-06 | Generated inputs for a fixed goal are byte-identical across two runs and two OSes. |
| AC-SYNTH-07 | The rendered prompt for a golden goal matches its snapshot and contains no child IR, file paths or env values. |
| AC-SYNTH-08 | `--locked` with a stale entry fails with `VL0702`; `--offline` with a cache miss fails with `VL0404`; neither constructs a provider. |
| AC-SYNTH-09 | No API key string appears in any output, log, fixture or artifact after a recorded build (grep test with a sentinel key). |
| AC-SYNTH-10 | `replay` provider reproduces a recorded build byte-for-byte (same artifacts, same lock). |
| AC-SYNTH-11 | Against a mock Ollama server, the `ollama` provider sends the reply JSON Schema as `format` with temperature 0 and no streaming, and an accepted artifact records `"provider": "ollama"` and `<model>@<digest>` as `model_version`. |
| AC-SYNTH-12 | Changing the digest behind the same Ollama tag, with an up-to-date lock, makes zero requests; after deleting the store entry, the next build misses on the new `synthesis_key`. |
| AC-SYNTH-13 | An unreachable Ollama server yields `VL0404`; a model the server doesn't have yields `VL0405` with an `ollama pull` hint. |
| AC-SYNTH-14 | The `external` `synthesize` request body for a golden goal matches its snapshot, validates against the committed request schema, and contains no file paths, environment values or API keys. |
| AC-SYNTH-15 | An `external` backend returning a hostile body (a `call` node, an unknown builtin) is rejected with `VL0402` in both cases (D-63); nothing is stored. |
| AC-SYNTH-16 | A reply that is not JSON, a body over the cap, a `5xx` after the transport retries, a redirect and an `{"error"}` reply each fail with `VL0406` naming the goal and backend, with the tail of the response body as a note; the lock is unchanged and no synthesis retry is made. A refused connection or a timeout is `VL0404`; a `401` or `403` is `VL0405` naming `VELME_EXTERNAL_TOKEN`. |
| AC-SYNTH-17 | With `max_retries = 1`, a second `external` request carries the first reply and its diagnostics in `attempts`; with the default 0, a rejected reply fails the goal after one request. |
| AC-SYNTH-18 | With sentinel values in `VELME_API_KEY` and `ANTHROPIC_API_KEY`, the `external` service receives neither and neither appears in any output or file; the `VELME_EXTERNAL_TOKEN` token is sent only as `Authorization: Bearer` and appears in no output, fixture or `replay.json`; a URL named in the project `velme.toml` is not used. |
| AC-SYNTH-19 | A fully cached build (every goal fresh in the lock) with `--provider ollama` or `--provider external` sends no request to the server, and resolves no model digest / runs no `describe` (D-57). |
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
| AC-SYNTH-31 | An `external` backend replying `{"pending": "ticket 42"}` with `max_retries = 3` fails the goal with `VL0408` showing "ticket 42" after exactly one `synthesize` request; nothing is stored, the lock is unchanged, another goal in the same file still builds, and the exit code is 2. When the backend then replies `{"body"}` to the same request, the next build accepts and locks it. |
| AC-SYNTH-32 | With `max_retries = 1`, a first `external` reply whose body fails a check and a second reply `{"pending"}` give `VL0408` with a note naming the failed check. An empty pending text gives `VL0406`. |
| AC-SYNTH-33 | `BuildPlayerSummary` calls `FindBadge`, which fails with `VL0403`: `BuildPlayerSummary` ends with `VL0409` naming `FindBadge` and its code as a note, with no provider call made for `BuildPlayerSummary` (D-56). |
| AC-SYNTH-34 | A build with several lock misses sends exactly one Ollama digest resolution (or `external` `describe`) request, on the first miss, reused for every later request in the build (D-57). |
| AC-SYNTH-35 | A scripted watchdog timeout (`VL0603`) during verification ends the goal with `VL0603` and exit 3 after exactly one provider call, never as a rejected candidate; nothing is stored, and its parent ends with `VL0409` (D-51, D-93). |
| AC-SYNTH-36 | An `external` URL that is `http` to a host other than `localhost`, 127.0.0.0/8 or `[::1]`, has another scheme or user information, or does not parse fails with `VL0902` before any contact; a redirect from the service is not followed (D-101). |
| AC-SYNTH-37 | The test-input generator reproduces golden vectors V1–V3 of §7.4 exactly, stage counts included (D-96). |
| AC-SYNTH-38 | A recorded fixture holds request hashes, replies and usage but no plan text; replaying it with one edited request, or with one request more than recorded, fails with `VL0404` naming the key and attempt (D-94). |
| AC-SYNTH-39 | With two goals needing synthesis and an unreachable provider, the first ends with `VL0404` after its transport retries (waits 1 s and 2 s on the injected clock) and the second ends with `VL0404` with no request made (D-93, D-95). |
| AC-SYNTH-40 | Only `body` is read from a reply: a scripted reply that also spells the goal name, versions, `inputs`, `output` or `types` wrongly builds the same IR as a bare `{"body"}` reply (D-103). |
| AC-SYNTH-41 | A reply that is not JSON, or has no `body`, is a failed attempt with `VL0401` in Velme's words; the help of `VL0403` names the listed builtins only for a rule about a built-in, not for a schema, type or size rule (D-103). |
| AC-SYNTH-42 | The rendered prompt shows a check as written and never as lowered IR, says checks are not the body and that a dotted name is never written, and each of its worked examples validates as a body for the goal it describes (D-104), in both reply formats: in `compact` the snippets and examples are spelled with the aliases and still expand. |
| AC-SYNTH-43 | After a reply that reads an input as a `local`, uses a dotted name, names a field or an input that isn't there, the next request's diagnostic lists the goal's inputs (and fields of the record) with types and the node that reads them, and repeats none of the reply's text (R-SYNTH-49, D-104); a composite lists its call results too, `add` on Text points to `concat` and `gt` on Text does not, a built-in named like an operator points to the `binary` or `unary` node and any other unknown built-in gets the callable built-ins, and in `compact` the nodes are named as that format spells them. |
