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
    fn input_version(&self) -> &str;         // prompt_version + request options (LLM providers, R-SYNTH-40) or the external `input_version` (R-SYNTH-40)
    async fn complete(&self, request: &SynthRequest, limits: &SynthLimits)
        -> Result<SynthReply, ProviderError>;
}

pub struct SynthRequest { pub request_version: String, pub ir_version: String, pub builtins_version: String,
                          pub task: TaskKind, pub goal: String,
                          pub signature: Signature, pub types: Vec<RecordType>, pub locals: Vec<LocalBinding>,
                          pub plan: String, pub checks: Vec<CheckItem>, pub examples: Vec<Example>,
                          pub budget: Budget, pub builtins: Vec<BuiltinSig>,
                          pub attempts: Vec<AttemptFeedback>,                // earlier replies + diagnostics (§5)
                          pub output_schema: serde_json::Value,              // that format's reply schema: goal body or question (R-SYNTH-10, §3.2)
                          pub reply_format: Option<ReplyFormat> }            // external only, absent for ir-json (§3.2, D-156)
pub struct SynthReply  { pub reply_json: String, pub usage: Usage, pub latency: Duration }
pub enum  ProviderError { NotConfigured, ModelMissing(String), KeyRejected, TokenRejected,
                          Rejected { status: Option<u16>, message: String },     // R-SYNTH-07, D-150: a 4xx no other variant names
                          Unavailable(String), RateLimited { retry_after: Option<Duration> },
                          Refused(String), Timeout, Malformed(String),
                          FormatUnsupported { backend: String, offered: Vec<String>, asked: String }, // R-SYNTH-26, D-156
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
| `anthropic` | hosted LLM (D-14) | Messages API, no `temperature` sent (D-150); output through two tools, `write_goal` (input: `{"body": <expression>}`, D-103, or `{"body": string}` in `notation`, R-SYNTH-55) and `ask_question` (input: the question object), with `tool_choice` `auto` and a system prompt telling the model to call exactly one (R-SYNTH-44) |
| `ollama` | local LLM (D-41) | Ollama chat API at the configured URL, `format` set to the reply JSON Schema (R-SYNTH-10; `{"body": string}` or the question object in `notation`), temperature 0, no streaming; no API key |
| `external` | human- or tool-written goal bodies (D-42) | speaks the §3.2 protocol over HTTP and JSON to a standalone service the user starts, at the URL the user configures (D-101); Velme starts no process |
| `replay` | integration tests, golden builds, CI | reads `<replay_dir>/b3-<hex>.json`, named from the synthesis key as store files are (R-SYNTH-43); missing fixture → `VL0404` naming the key; `VELME_SYNTH_RECORD=1` with a live provider writes fixtures |
| `scripted` | unit tests of the retry loop and pipeline | a queue of replies/errors: in memory from library tests, or from the script file in `VELME_SYNTH_SCRIPT` in a `velme-cli` built with the `test-provider` feature, never in release builds (`tooling/40` §5.2) |

**R-SYNTH-06** Live-provider runs are never part of the default gate (D-13). In v0.1 they are a manual CLI run at the
MVP gate (`delivery/51` §2, D-148); `VELME_LIVE_LLM=1` is reserved for live-provider tests after v0.1.
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
candidates that failed validation, questions, pending and refused replies included; transport retries are not exchanges,
and neither is a request the provider rejects (`Rejected`, like `KeyRejected` and `NotConfigured`), so its goal replays
as the missing-entry `VL0404` — as canonical JSON, overwrites the goal's whole fixture file each time it synthesizes that
goal, and writes `replay.json`.
**R-SYNTH-44** `anthropic` sends `tool_choice: {"type": "auto"}` and a system prompt telling the model to answer only
by calling exactly one of `write_goal` and `ask_question`, since current models reject a forced `tool_choice` and a
`temperature` (D-150); the tool's input is the reply (R-SYNTH-10; `write_goal` takes the body in the requested
`reply_format`, R-SYNTH-36, §4.2), text blocks beside the one tool call are ignored, and
a reply with no tool call, both tools or anything else is `Malformed`. In a retry, each carried reply that is a JSON
object goes back as the `tool_use` block it was (`ask_question` for a question object, else `write_goal`), and the user
turn after it starts with the matching `tool_result` block holding the feedback; a carried reply that isn't one stays a
text turn, and so does its feedback (R-SYNTH-11, D-150). Tests reach a
mock server through a base URL set only by the test-only constructor in `velme-test-support`, never by a config key
or environment variable (D-98).
**R-SYNTH-07** `ProviderError` mapping: `NotConfigured` → `VL0405`; `KeyRejected` (the API answered 401 or 403 to a key
that is set) → `VL0405` worded "the API key was rejected" (`reference/90`); `TokenRejected` (an `external` service
answered 401 or 403) → `VL0405` worded "the external backend rejected the token", with a hint naming `VELME_EXTERNAL_TOKEN`; `File` (a replay file that is a link, is
not a regular file, is too large or can't be read or written) → `VL0901`; `Unavailable`/`Timeout`/`RateLimited` after
transport retries → `VL0404`; `Refused`/`Malformed` count as a failed attempt (§5) with `VL0401`, in Velme's own
wording, never the provider's text (R-SYNTH-22, D-93); a `Malformed` reason is one id of a closed list, each with
Velme's own words: `too-large` (the response passed the size limit), `response-not-json` (the response wasn't JSON,
nested too deeply or repeated a key), `cut-off` (`anthropic` `max_tokens`, `ollama` `length`), `not-one-tool` (no tool
call, or more than one), `unknown-tool` (a tool other than the two, or an input that isn't an object), `no-message` (an
`ollama` response without one), `reply-not-json` (the tool input or `ollama` reply wasn't JSON, nested too deeply or
repeated a key), `reply-not-object` (an `ollama` reply that isn't an object). The reason stays the attempt's detail
through the retry turn, the learner's `VL0403` message and the log's `cause` (R-SYNTH-23), never collapsed to a generic
message (D-153), but all `Malformed` attempts share one cause whatever their reasons (R-SYNTH-31, so `stop_on_repeat`,
R-SYNTH-37): a fixture keeps only the variant (R-SYNTH-43), so a replayed `Malformed` has no reason and must still stop
where the live build stopped; `Rejected` (`anthropic` or `ollama` answered a generation
request, `/v1/messages` or `/api/chat`, with a `4xx` that provider maps to no other variant: for `anthropic` any but
401/403 `KeyRejected`, 404 `NotConfigured`, 408 and 429; for `ollama`, which sends no key, any but 404 `ModelMissing`,
408 and 429, so a 401 or 403 from a proxy in front of it is `Rejected`; a `4xx` to Ollama's `/api/tags` stays
`Unavailable`) → `VL0405` worded "the provider rejected the request (HTTP {status})", not retried, with the API's own
error message (`error.message` of Anthropic's error envelope, `error` of Ollama's) as a quoted note, the key taken out,
its control and format (`Cf`) characters other than bidi controls made spaces, bidi controls kept and escaped when
shown (R-CLI-17, D-108), and bounded as `BackendFailed`'s reason is (R-SYNTH-33), and no note when the body has none: the one provider error text a
diagnostic shows besides `external`'s (a question, R-SYNTH-33, is shown too) (D-150); `BackendFailed` and `FormatUnsupported` (R-SYNTH-26) → `VL0406`, not retried; `Pending` → `VL0408`,
not retried (R-SYNTH-41); `Internal` (a Velme bug, such as a request with no hash) → `VL0607`, not retried.
**R-SYNTH-45** After one goal ends with `VL0404`, or with `VL0405` from a call (a rejected key or a model the API
doesn't know; not a `Rejected` request, which may be that goal's alone, D-150) or `VL0901` from a replay file, the build contacts the provider no more: every later goal that reaches
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
(D-57). Under `--offline` the identity step does not run at all: no provider is asked for anything, so a goal that isn't
fresh in the lock is `VL0404` and the store is not consulted (`tooling/40` R-CLI-05, D-106).

### 3.2 External backend protocol (D-42, D-101)

The `external` provider lets a person or any tool supply the implementation instead of an LLM. The backend is a
standalone service that the user starts; Velme starts no process. Velme sends HTTP requests with JSON bodies to the
service's base URL (`tooling/40` R-CLI-13) and reads one JSON document from each response. The request body follows the
committed `velme-synth-request` JSON Schema, generated from the Rust types like the IR schema (21 R-IR-20).

| Endpoint | Velme sends | Backend replies |
|---|---|---|
| `GET /v1/describe` | no body | `{"backend": "<name>", "backend_version": "<version>"}`, and optionally `"reply_formats": ["<name>", ...]` (R-SYNTH-26) |
| `POST /v1/synthesize` | the canonical `SynthRequest` as the body (`Content-Type: application/json`) | `{"body": <expression>}` (`{"bo": ...}` in `compact`, R-SYNTH-36), `{"question": "<text>"}`, `{"pending": "<text>"}` or `{"error": "<reason>"}` |

When `reply_format` (R-SYNTH-36) is not `ir-json`, the request holds one more member, `"reply_format": "<name>"`, and the backend
answers in that format (the `body` key spelled as the format spells it); with `ir-json` the member is left out, so the
request is the same bytes as without the option (D-156). `output_schema` is that format's reply schema, so a `compact`
request does not carry the IR schema: its body is the object under the alias of `body`, or a question. Within
`request_version` `0.1` Velme adds only optional members, absent by default and sent only to a backend that advertises
support for them (`reply_formats` is how a backend does it for `reply_format`), so a backend written against the first
`0.1` keeps working (D-156). The endpoints are relative to the base URL, so a service may live under a path prefix. A service may add fields to its
replies (unknown keys are ignored, as in `describe`); a change that isn't compatible with this protocol goes to `/v2`. `SynthRequest` carries its own
`request_version` (`"0.1"`); the `/v1/` prefix versions the endpoints.

**R-SYNTH-26** `describe` runs once, on the first lock miss in a build (D-57), and is skipped entirely for a fully
cached build; its `reply_formats`, when present, is a list of reply-format names (R-SYNTH-36) the backend answers in, and
when absent means `["ir-json"]`; a name Velme doesn't know is ignored, a `reply_formats` that is not a list of strings is `VL0406`,
and the list is part of neither the identity nor any key. When `[synthesis] reply_format` is not `ir-json` and the list
lacks it, `describe` ends the goal with `VL0406` before any `synthesize` request: "The backend `X` couldn't build `Goal`: it answers
only in `ir-json`, and velme.toml asks for `reply_format = "compact"`.", help "remove `reply_format` from `[synthesis]`, or use a backend
that lists it" (`VL0406`, not `VL0902`: the settings are valid, it is the backend that cannot do what they ask). Velme never
falls back to another format (D-156). Its `model()` is `<backend>@<backend_version>` (as Ollama's is `<model>@<digest>`) and enters `synthesis_key`, so a new
backend or version is a cache miss but never makes a lock stale (runtime/32 R-ART-03). `backend` and `backend_version` must each be 1..=128 Unicode scalar
values after whitespace runs collapse to one space, and must hold no control character, Unicode format (`Cf`) character
or bidi control (D-47): those are refused, not cleaned away. Any other key of the reply is ignored, so services may add fields; a `describe` that
fails in any R-SYNTH-28 way or breaks this is `VL0406` (D-98).
**R-SYNTH-27** A `synthesize` reply's `body` is handled exactly like an LLM reply, in the request's `reply_format` (a `compact` body is expanded, and one keyed `body` is a failed attempt with `VL0401`, R-SYNTH-36; D-156): Velme completes it into an IR goal
(R-SYNTH-10), then full validation (21 §6) and verification (§6). The backend gets no trust the LLM doesn't get (INV-1). A `question` reply follows
R-SYNTH-32..33, as an LLM's does.
**R-SYNTH-28** Transport uses the same HTTP client and transport retries as `ollama` (R-SYNTH-12, D-101). A refused
connection, a DNS failure or a timeout is `Unavailable` or `Timeout` → `VL0404` after those retries, and the build then
contacts the provider no more (R-SYNTH-45); so does a body that stalls after its headers. A `429` is `RateLimited` → `VL0404` and a `408` is retried like a `5xx` (then `VL0406` if it persists). A `401` or `403` is `TokenRejected` → `VL0405` (R-SYNTH-07), not retried: "rejected the token" when a token was sent, "wants a token" (hint: set `VELME_EXTERNAL_TOKEN`) when none was.
A `5xx` after the retries, any other non-2xx status (a redirect included: redirects are never followed), a body above
2 MiB, a body that is not one JSON document, a reply that does not hold exactly one of the four kinds above (any other key is ignored; in `compact` the body kind is `bo`, and `bo` and `body` count as two kinds, so a reply holding both is of no known kind, while one holding only `body` is the failed attempt of R-SYNTH-27), a reply whose `body` holds the
bearer token ("the reply contains your token"), or an `{"error"}` reply is `BackendFailed` → `VL0406` naming the goal and backend, with the last 4 KiB of the response body in the notes,
cleaned and escaped like any untrusted text (T-13). Proxies and TLS: for a host that isn't loopback, the client honours
`ALL_PROXY`, `HTTPS_PROXY` and `HTTP_PROXY` from the environment through a CONNECT tunnel; a loopback host is never
proxied. TLS trusts only the bundled `webpki-roots`, so a TLS-inspecting proxy or a private CA fails with `VL0404`; a
user-level `external_ca_file` setting (`tooling/40` §5.1, D-105) adds one PEM file of roots to the bundled ones, for `external` only. `external_timeout_secs`, a user-level setting with a default of 30 s, bounds each request, `describe` included.
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
ids + BLAKE3 of all the template bytes (an edit to one re-keys every goal; the notation's `notation.txt` is not a template here but an option member, so an edit to it re-keys only `notation` builds, §4.2, D-157), the compact alias table (R-SYNTH-36) and the schema summary lines (R-SYNTH-35), so
an edit to any of them, or to an option that shapes the prompt (R-SYNTH-40), changes synthesis keys (D-11, D-97). It does
not cover the code that renders a template or the providers' tool descriptions and request settings (the `anthropic`
system prompt, `tool_choice`, `temperature`, the shape of retry turns): like any compiler code change, an edit
there re-keys nothing (`runtime/32` R-ART-04). Both LLM providers share the templates. The table below is also the content of `SynthRequest`; the external protocol (§3.2)
sends the same fields as structured JSON.

| Section | Contents | From |
|---|---|---|
| Header | `prompt_version`, `ir_version`, `builtins_version` | constants |
| Output contract | "return `{"body": <expression>}`, the goal's body as one IR expression node, which is all Velme asks for (D-103); use only listed builtins; only if the plan leaves open a choice that changes the result, return `{"question": …}` instead" + the reply schema or its summary (R-SYNTH-35). In `compact` the body key is spelled by its alias (R-SYNTH-36); in `notation` the body is the notation text (§4.2). The contract names the reply's content, never how it is delivered: no "reply with JSON", no "call a tool", which the provider's own request says (R-SYNTH-44, `ollama`'s `format`) (D-153) | `velme-ir` / `velme-synth` |
| Rules and worked example | in the reply's format (the `compact` aliases when asked, R-SYNTH-36; in `notation`, the §4.2 forms in place of nodes): one line each: an input is read with an `input` node, a record's field with a `field` node and never a dotted name, `local` only for a lambda's parameter or a call result; checks and examples describe the result and are never the body, and `result` is not a name the body can read. Arithmetic and comparison are `binary` ops and never builtins, and the `unary` ops are listed too (both from the IR schema). Then three fixed worked examples, for a made-up goal that no example of the repository uses, each a valid body and together showing a `builtin` call, a top-level `binary` node and a lambda's `local` (D-104) | template |
| Allowed builtins | name + signature of each catalog entry | `velme-builtins` |
| Goal | task kind (`leaf` / `composite-tail`) and signature `Name(params) -> Output` | HIR |
| Types | every reachable record type with fields | HIR |
| Locals | composite only: each call binding `name: Type = Child(args)` — child **signatures** only, never child IR | HIR (D-5, D-11) |
| Plan | normalized plan text (D-21), fenced and labelled as untrusted user description (§9) | HIR |
| Checks | the source text of each check, and nothing else: its lowered form reads like a body and models copy it (D-104). The `SynthRequest` still carries the lowered form for `external` (§3.2) | HIR |
| Examples | the first `max_prompt_examples` `examples:` items in source order, with literal values (R-SYNTH-38) | HIR (D-7) |
| Budget | effective budget (runtime/30 §7) | HIR + system caps |

**R-SYNTH-08** The prompt contains nothing outside this table: no file paths, no other goals' plans, no environment,
no user identity (tooling/41). The one fixed text beside it is `anthropic`'s system prompt asking for one tool call
(R-SYNTH-44), which holds nothing from the request and, like the tool descriptions, is not hashed (§4, D-150).
**R-SYNTH-09** Output is constrained to the reply schema (a `{"body"}` object over the IR expression schema, its
`compact` renaming, or in `notation` a `{"body"}` object whose `body` is a string, §4.2; or a question object, R-SYNTH-10) where
the provider supports it; Velme still runs its own full validator on every reply — a provider's schema
guarantee is never trusted.
**R-SYNTH-10** The reply is one object `{"body": <expression>}`, `body` being one IR expression node (`compiler/21` §3), or one
question object `{"question": "<text>"}` (R-SYNTH-32). In `compact` the body is the aliased tree under the body's
alias (R-SYNTH-36); in `notation` it is `{"body": "<notation>"}`, a string that §4.2 lowers to one expression node
before anything below applies. Only `body` is read from it, and Velme writes the rest of the
IR goal itself from the request: `ir_version` and `builtins_version` (§3.1), `goal`, `inputs` and `output` from the
signature, and `types` from the request's `types`, which are exactly the record types R-IR-01 reaches (`compiler/21`
D-84); `calls` is left out and the compiler's are joined in (R-IR-02). The assembled goal then passes the full validator
(`compiler/21` §6), so every rule that judges a body still judges it, and no reply can differ from its goal's signature
(D-103). Any prose, markdown fence, partial JSON, a reply with no `body`, one holding both a `body` and a `question`
(neither wins) or another shape is a failed attempt with `VL0401`, as is a number in the `body` that canonical JSON can't hold, or in `notation` a `body` that is not a string; a `body` that is not an expression, or repeats a key, fails at validation stage 1, at a JSON path under `/body`. The reply schema and
the summary of R-SYNTH-35 leave out the `call` node, which only the compiler's `calls` may hold (a body reads a call's
result as a `local`).

### 4.1 Token cost (D-44)

The options below live in `[synthesis]` (`tooling/40` §5.1) and apply to the LLM providers only, except `reply_format`, which
`external` takes too (D-156); `external` always gets the full request (§3.2, R-SYNTH-30). None of them changes which IR is accepted: every reply still passes the full
validator and verification (INV-1, INV-2).

**R-SYNTH-34** The template renders the sections in the order of the §4 table. Header, output contract and allowed
builtins are the same for every goal and attempt with the same options, so they form a fixed prefix. With
`prompt_cache = true` (default) `anthropic` marks the end of that prefix as a cache breakpoint; a prefix below the
model's minimum cacheable length is simply sent uncached. Caching changes price, never the reply, so it does not enter
any key.
**R-SYNTH-35** `schema_in_prompt = "summary"` (default) puts one generated line per IR expression node kind and the question
object into the output contract and sends the reply schema only through the provider's constraint (R-SYNTH-05,
R-SYNTH-09); `"full"` also puts the whole schema in the prompt, for models that follow it better when they can read it.
In `notation` the output contract carries the notation's lines (§4.2) in place of the node lines, and the reply schema
is the small `{"body": string}` one.
**R-SYNTH-36** `reply_format = "ir-json"` (default) asks for canonical IR JSON, `{"body": …}`. `"compact"`
(experimental, D-153) asks for the same tree with every property name and node `kind` tag, the reply's `body` key
included, replaced by a short alias from a fixed table in `velme-synth`, generated together with the reply schema and
versioned with it. An alias is always shorter than its name, and a name with no shorter free alias keeps its own
spelling. The aliases are pairwise distinct and none equals another canonical property name or `kind` tag, so no key
of a reply reads two ways (an alias table where `args` becomes `arg` while `arg` becomes `ar` is a defect). The output
contract, the reply schema and the parser all spell the body key by its alias, and the parser reads only that
spelling: a compact reply keyed `body` is a reply with no body (`VL0401`). `velme-synth` expands a compact reply into
canonical IR (a pure renaming; an unknown alias is `VL0401`) before validation, so the validator, fingerprints,
artifacts and lock only ever see canonical IR (D-21). Retry turns carry each earlier reply exactly as it was received,
never compressed again from the expanded IR, and spell diagnostic JSON paths with the aliases. `"notation"`
(experimental, D-152) asks for the body as notation text and lowers it to IR (§4.2). Compact is removed in `0.2` if
the notation becomes the default (D-153). An `external` backend is asked for the format the same way: the request names it
(§3.2) and the reply is read by the same code (D-156).
**R-SYNTH-37** `retry_history = "latest"` (default): a retry turn carries only the latest failed reply plus the primary
diagnostic (R-SYNTH-31) of every earlier attempt, so later attempts cost about as much as the second. `"all"` carries
every earlier reply, as R-SYNTH-11 describes. With `stop_on_repeat = true` (default), the loop stops when two attempts
in a row share a cause (R-SYNTH-31), and the goal fails with `VL0403` at once.
**R-SYNTH-38** `max_prompt_examples` (default 8, 0..=64) bounds the examples sent. Every example still runs locally
(§6); a candidate that fails one left out gets it, with its values, in the next retry turn.
**R-SYNTH-39** `retry_model` (optional) is the model used for retries; attempt 0 uses `model`. When set, `model()` is
`<model>+<retry_model>` (Ollama: each with its digest), so the pair enters `synthesis_key` (runtime/32 §2).
**R-SYNTH-40** For LLM providers, `input_version` is `prompt_version` + BLAKE3 of `schema_in_prompt`,
`reply_format`, `retry_history` and `max_prompt_examples`, because they change what is sent, and with `reply_format =
"notation"` the notation's grammar version and the hash of `notation.txt` too (R-SYNTH-55), because they change the IR
a reply gives and the prompt sent. Changing an option only
changes `synthesis_key`; lock staleness uses `contract_key` (D-26), so it never re-synthesizes a locked goal. For `external`,
`input_version` is `request_version`, followed by `+reply_format=<name>@<format version>` when the format is not `ir-json` (D-156). The
format version of `compact` is the first 12 hex digits of the BLAKE3 of its alias table (R-SYNTH-36), the table `prompt_version` hashes
(D-97); that of `notation` is the notation's grammar version (R-SYNTH-55). A change to either re-keys every build that asks for the format.

### 4.2 Reply notation (experimental, D-152)

With `reply_format = "notation"` the model writes the goal's body as text in a small Python-shaped notation,
`{"body": "<notation>"}`, and `velme-synth` lowers it to one IR expression (`compiler/21` §3) before the unchanged
validator, which judges it like any other body (INV-1). The notation is defined only by the tables below, never by
Python's semantics: a form with no row is rejected, and a form with a row means that row's IR. So numbers are exact
decimals (R-TYP-08), `/` is Velme's division (D-36), `round` rounds ties away from zero (AC-BLT-04), and the prompt's
notation lines say so. An `external` backend may be asked for it too (§3.2, D-156). Artifacts, the lock,
fingerprints and `--emit ir` keep canonical IR, and no command shows the notation to a user in v0.x.

**R-SYNTH-50** Text. The body is at most 64 KiB of UTF-8, checked before anything else is read. Names are ASCII
letters, digits and `_`, not starting with a digit. Outside a Text literal or a comment the body holds only names,
digits, spaces, line ends and ``+ - * / = ! < > ( ) [ ] , : . # " '``, besides the characters a rule names (`;` is
`notation-4`, ``% @ & | ^ ~`` are `notation-7`, `{ }` are `notation-8`); a Text literal or a comment holds any character
but a control character (Unicode general category `Cc`). The augmented operators ``+= -= *= /= //= %= **=`` are
each one token, `notation-4` wherever they stand (R-SYNTH-57); `==` is one token, never `=` twice. A line ends at `\n`
(`\r\n` counts as one; a lone `\r` is a control character). Indentation is spaces only: the top level's is that
of the body's first statement (usually none), and a block's lines share one
indentation deeper than its header line. A line may continue inside `(` or `[`, where line ends and indentation are not
read, never after a `\`. `#` starts a comment to the end of the line; blank and comment-only lines are ignored. A number
literal is `0` or digits not starting with `0`, with an optional `.` and one or more digits (`12`, `0.05`), kept as
exact decimal text and never read through a binary float (R-TYP-08); a letter, digit, `_` or `.` straight after one,
and a `.` straight before a digit, are `notation-3`. A Text literal is `"…"` or `'…'` on one line, with the escapes
`\\ \" \' \n \t` and `\u` plus 4 hex digits (either case) naming a Unicode scalar value. A word is a name or a
keyword. The keywords are `and elif else False for if in is lambda None not or return True`, and a keyword is still
read as a name (`WORD` in R-SYNTH-57) in two places: straight after `.` (`x.if`) and as a keyword argument's name, a
word directly before a lone `=` inside a call's brackets (`T(in=1)`; `==` is not a lone `=`). Python's other keywords
(`while def class import from try except finally with as pass break continue raise assert del global nonlocal yield
async await`) and `match` are reserved in one place only: as the first token of a statement when the next token is
neither `=` nor `.`, where they are `notation-4` (`while c:`, `def f():`, `import math`, `match x:`). Everywhere else
they are names (`bond.yield`, `T(from=x)`, an input named `class`, `class = 1`), as is any other word (`type` and
`print` included). An input or call binding named `return`, `else`, `elif`, `lambda`, `None`, `True` or `False` can't
be read in the notation, since those words are keywords wherever a name is read (Q-32); Velme's own keywords already
keep the rest of the list out of Velme names (`language/10` R-SYN-05).

**R-SYNTH-57** Grammar. Over the tokens of R-SYNTH-50, with `NEWLINE` at each line end outside brackets and
`INDENT` / `DEDENT` where the indentation grows or returns to an open block's, a body is exactly:

```ebnf
body        = block ;
block       = { assignment | open_if } final ;
assignment  = NAME "=" expr NEWLINE ;
open_if     = "if" expr ":" branch { "elif" expr ":" branch } ;   (* no else: the rest of its block is its else *)
final       = "return" expr NEWLINE
            | "if" expr ":" branch { "elif" expr ":" branch } "else" ":" branch ;
branch      = "return" expr NEWLINE | NEWLINE INDENT block DEDENT ;

expr        = disjunction [ "if" disjunction "else" expr ] | lambda ;
lambda      = "lambda" NAME { "," NAME } ":" expr ;
disjunction = conjunction { "or" conjunction } ;
conjunction = inversion { "and" inversion } ;
inversion   = "not" inversion | comparison ;
comparison  = sum { comp_op sum } ;
comp_op     = "==" | "!=" | "<" | "<=" | ">" | ">=" | "in" | "not" "in" | "is" | "is" "not" ;
sum         = term { ( "+" | "-" ) term } ;
term        = factor { ( "*" | "/" ) factor } ;
factor      = "-" factor | primary ;
primary     = atom { "." WORD } ;
atom        = NAME [ "(" [ arguments ] ")" ] | NUMBER | TEXT | "True" | "False" | "None"
            | "(" expr ")" | "(" generator ")" | tuple
            | "[" [ expr { "," expr } [ "," ] ] "]" | "[" generator "]" ;
tuple       = "(" ")" | "(" expr "," [ expr { "," expr } [ "," ] ] ")" ;  (* only as the whole right operand of in *)
generator   = expr "for" NAME "in" disjunction [ "if" disjunction ] ;
arguments   = generator | argument { "," argument } [ "," ] ;
argument    = WORD "=" expr | expr ;
```

`NAME` is a name of R-SYNTH-50, a reserved word included; `WORD` is a name or a keyword (R-SYNTH-50's two places).

A statement is told by its first tokens, the first of these that applies:
1. `return`, `if`: that statement. `elif` or `else` continues the `if` whose branch just ended at the same
   indentation; with none to continue it is `notation-13`.
2. `for`, or a reserved word (R-SYNTH-50) whose next token is neither `=` nor `.`: `notation-4`.
3. A `NAME` then `=`: an assignment.
4. A `NAME` then `,` or `:`: `notation-4` (unpacking, `a, b = …`; an annotation, `x: int = 1`).
5. Anything else: an expression is parsed, and the statement is a bare expression.

Then, after the expression of a `return`, an assignment, a bare expression or an `if`/`elif` condition: a lone `=` is
`notation-4` (`x = y = 1`, `x.y = 1`), and a `,` is `notation-8`, a tuple (`return a, b`, `x = a, b`). A bare
expression that is the last statement of its block (its logical line is followed by a `DEDENT` or the end of the
text) is `notation-5`, its block lacking `return`; any other bare expression is `notation-4`. An augmented operator is
`notation-4` wherever it stands (R-SYNTH-50). On a branch's header line only `return e` may follow the colon (else
`notation-5`, `if c: x = 1`), and a branch with no lines is `notation-5`. `or`, `and`, `+ -` and `* /` group to the left
(`a - b - c` is `(a - b) - c`), comparisons chain (R-SYNTH-52), and `a if c else b if d else e` groups to the right;
parentheses group and lower to no node. The parser keeps each operator chain flat and lowering nests it. The parser's
nesting counts every bracket, indented block, `not`, unary `-`, `lambda` and conditional `else` open at once (the top
level is no block: its statements stand at nesting 0, an `if` branch's lines at 1); it also counts each operator chain
held in a chain of another precedence (`a or b and c` is 1), so no syntax tree is deeper than the parser's nesting and
lowering never recurses past the cap: past 128 it stops with `notation-12`.

The parser also recognizes these forms outside the grammar, so that each is `notation-8` rather than `notation-13`,
placed at the token named: a subscript or slice, at a `[` straight after a primary (`x[i]`); a call on anything but a
`NAME`, at a `(` straight after any other primary (`x.f(…)`, `(f)(x)`, `f(a)(b)`); `*` or `**` at an argument's start;
a `,` after a generator's `NAME` (`for a, b in xs`); a second `for`, or an `if` after the generator's own `if` clause;
a `lambda` or generator outside R-SYNTH-52's places, at `lambda` or the generator's `for`; and a `tuple` that is not the
whole right operand of `in` or `not in` (`(a, b)`, `()`, `(a,)` elsewhere, `x in (a, b) + ys`), at its `(`. A tuple in
its place lowers exactly as the list `[a, b]` would (R-SYNTH-52). This grammar and the tables of §4.2 are grammar
version `1` (R-SYNTH-55).

**R-SYNTH-51** Statements. A body is a block: zero or more assignments and `if`s without `else` whose branches end in
`return` (early returns), then one terminal statement (`return`, or an `if` with `else` all of whose branches end in
one), and nothing after it. An `if … elif …` with no `else` takes the rest of the block as its final `else`.

| Notation | IR |
|---|---|
| `name = e`, then the rest `R` of the block | `let` binding `name` to `e` with body `R`; consecutive assignments join one `let`'s `bind`, in order |
| `return e` | `e` |
| `if c:` B1 `elif c2:` B2 … `else:` Bn | `if` with `cond` c, `then` B1, `else` a nested `if` per `elif`, the last `else` Bn |
| `if c:` B1 (`elif` …) with no `else`, then the rest `R` of the block | `if` with `cond` c, `then` B1, a nested `if` per `elif`, the last `else` R (an early return); R must exist |

Each branch is an indented block, or a single `return e` on the header line after the colon. A name is bound once: an
assignment, a lambda parameter or a comprehension variable never reuses an input, a call binding or a name already
bound in scope, and a name is never assigned again (`let` has no shadowing, `compiler/21` §3). In scope means an
earlier assignment of the same block or an enclosing one, or an enclosing lambda or comprehension: two branches, or two
comprehensions side by side (`sum(x.a for x in xs) + sum(x.b for x in xs)`), may use one name.

**R-SYNTH-52** Expressions. Operator precedence and associativity are Python's for the operators listed, loosest
first: `lambda`, `a if c else b`, `or`, `and`, `not`, comparisons (with `in`, `not in`, `is`, `is not`), `+ -`,
`* /`, unary `-`, then field access and calls. Parentheses group.

| Notation | IR |
|---|---|
| number literal; a unary `-` whose operand is a `NUMBER` token (`-2`, `- 2`) | `literal` Number with that exact decimal; the `-` folds into one negative literal. Only that innermost `-` folds: `--2` is `unary` `neg` of the literal `-2`, and `-(2)` is `neg` of `2`, since parentheses end the fold |
| `"t"` | `literal` Text |
| `True`, `False` | `literal` Boolean |
| `None` | `literal` Nothing (`nothing`) |
| an input's name | `input` |
| an assigned name, a lambda parameter, a comprehension variable, a call binding (composite) | `local` |
| any other name | `local` of that name, which the validator rejects (`names-8`) with the R-SYNTH-49 hint |
| `R(f=e, …)`, `R` a record type of the request | `record` of type `R`; keyword arguments only, each a field |
| `e.f` | `field` |
| `[e, …]` | `list`, its `of` by R-SYNTH-53 |
| `a + b`, `a - b`, `a * b`, `a / b` | `binary` `add` `sub` `mul` `div`; Number only, so `+` on Text fails the validator (`types-15`) with the hint to use `concat` (R-SYNTH-49) |
| `a == b`, `!=`, `<`, `<=`, `>`, `>=` | `binary` `eq` `ne` `lt` `le` `gt` `ge` |
| `a < b < c`, any chain | `binary` `and` of `a < b` and `b < c`, left-nested for longer chains (`and(and(a < b, b < c), c < d)`); each comparison by its own row. `b` is lowered once, and its IR is copied verbatim into the second comparison, with the same `__v<n>` names and the same spans; the copy's nodes count toward the 10 000-node cap before it is made, so a copy past the cap is `notation-12` and never built (pure, so only its fuel counts twice) |
| `a and b`, `a or b`; `not a` | `binary` `and`, `or`, left-nested for `a or b or c`; `unary` `not`. Operands must be Boolean: a non-Boolean one, as in `if xs:`, fails the validator's type stage; there is no truthiness |
| `-a` | `unary` `neg` |
| `x is None`, `x == None`, `None == x`; `x is not None`, `x != None`, `None != x` | `binary` `eq` of `x` and the literal `nothing`, `x` on the left whichever side it was written on (the right `None` becomes the literal when both are `None`); `ne` for the negated forms. This is D-61's comparison: `false` for any present value, an empty list or Text included. On an `x` that can't be `nothing` the validator rejects it (`types-14`) with a notation hint (R-SYNTH-59) |
| `v in xs`, `v not in xs` | `builtin` `contains(xs, v)`, and `not` of it; a tuple `(a, b)` as `xs` lowers as the list `[a, b]` (R-SYNTH-57) |
| `a if c else b` | `if` |
| `len(x)` | `builtin` `length` |
| `unwrap_or(x, d)` | `unwrap_or` |
| `[e for x in xs]` | `map` over `xs`, parameter `x`, body `e` |
| `[x for x in xs if p]` | `filter` |
| `[e for x in xs if p]`, `e` not just `x` | `map` over the `filter` |
| `sum(e for x in xs)`, optionally `if p` | `builtin` `sum` of the comprehension's lowering |
| `all(p for x in xs)`, `any(…)`, optionally `if q` | `all` / `any` over `xs` (over the `filter` by `q`), body `p` |
| `all(xs)`, `any(xs)` | `all` / `any` whose body is the parameter itself, a reserved name (below) |
| `next((x for x in xs if p), None)` | `find` |
| `next((x for x in xs if p), d)` | `unwrap_or` of the `find`, default `d` |
| `sorted(xs, key=lambda x: k)`, optionally `reverse=True` or `reverse=False`, the two keywords in either order | `sort_by`, `descending` as given (default `false`) |
| `sorted(xs)`, optionally `reverse=` as above | `sort_by` whose key is the element itself, a reserved name (below) (Number lists only, D-59) |
| `reduce(lambda acc, x: e, xs, init)` | `reduce` |
| `name(e, …)`, any other name | `builtin` `name` with those arguments in order, its signature from the catalog (language/14); a name not in the catalog fails the validator (`names-11`) with the R-SYNTH-49 hint |

A generator `(e for x in xs if p)` appears only as the argument of `sum`, `all`, `any` and `next`, and a `lambda` only
in `sorted`'s `key` and `reduce`. A parameter the notation doesn't name (`all(xs)`, `any(xs)`, `sorted(xs)`) is
`__v<n>`, `n` counting from 0 in lowering order and skipping any that names an input or call binding of the request;
the IR puts no pattern on local names (`compiler/21` §3), and a notation name bound in that form is `notation-6`.
Lowering order is text order: a form is lowered before the forms that start after it. A chain's copied middle operand
(the chain row above) takes no new `__v<n>`: it keeps the names of the operand it copies.

A call `NAME(…)` is read in this order: one of the rows above when `NAME` is `len`, `unwrap_or`, `sum`, `all`, `any`,
`next`, `sorted` or `reduce` (`sum`, `all` and `any` take the generator rows only for a generator argument, so
`sum(xs)` is the catalog's `sum`); a `record` when `NAME` is a record type of the request; `notation-10` when `NAME`
is on that rule's list and not in the request's catalog, whatever its arguments; else a `builtin`, whose arguments are
positional only, so a keyword argument there is `notation-8`. The first of these is told by the `NAME` token alone, so
the parser checks those forms' shapes (`notation-8`, `notation-9`); the others need the request, so lowering checks
them (R-SYNTH-59). A
name is read in this order too: a binding in scope (R-SYNTH-51), then an input, then a call binding, then any other
name. Each lowered node keeps, beside the IR, the span of the text it came from (R-SYNTH-59): an expression's from
its first token to its last; a `let` from its first assignment to the end of its block; an `if` statement from `if`
to the end of its last branch, or for an early return to the end of the rest of the block. A node the lowering adds
(the `and` of a chain, the `not` of `not in`, a `__v<n>` local, the `filter` under a
comprehension's `map`, the `find` under `next`'s `unwrap_or`) has the span of the form it comes from.

**R-SYNTH-53** List types. A `list` node's `of` is the type expected where the list stands, set by R-SYNTH-58, which
is normative for it: the goal's output for a list a `return` gives, the other branch of an `if`, the other side of `==`
or `!=`, the record field it fills, a catalog parameter of one fixed type, `unwrap_or`'s `of` for its default, a
`reduce`'s own type for its `init`, the enclosing list's `of` for an item, and with none of these the first item's
type. The types come from the validator's own type rules (`compiler/21` §6), never from a second type checker. An
empty `[]` with no expected type is rejected (`notation-11`), never given a guessed type; there is no typed-empty form.

**R-SYNTH-58** List typing helper. R-SYNTH-53's types come from one function of `velme-ir`'s validator,
`infer_list_types`, which types nodes with stage 4's own per-kind rules, the same functions the validator calls (one
implementation, not a fork of it; D-157). It takes the lowered body and the validator's request (`compiler/21` §6),
sets the `of` of every `list` node by the table below (whatever lowering put there), and returns the JSON pointer of
every `list` it can give no type. It reports nothing else: an unknown name or a type error makes a type unknown and is
left to the validator. Lowering reports the pointer whose node's span starts first, ties going to the node met first
in a pre-order walk of the lowered tree, as `notation-11`.

| Where the list stands | Its expected type |
|---|---|
| the body; a `let`'s body; an `if`'s `then` or `else` | the goal's output; the `let`'s or `if`'s own; for an `if` with none, the other branch's type |
| a side of `eq` or `ne` | the other side's type |
| a record field | the field's declared type |
| a `builtin` argument | the parameter's type, when every catalog signature of that name and arity gives it one type with no type variable (`sum`'s `List<Number>`; not `contains`'s `List<T>`, nor `length`, which has two) |
| `unwrap_or`'s `default` | `T`, when its `of` is `T?` |
| `reduce`'s `init` | the `reduce`'s own |
| an item of a `list` whose `of` is known | that `of` |
| anywhere else: a `let` binding's value, a lambda's body, a collection node's `list`, an operand of another operator, a `field`'s `of` | none |

Each node is typed once, in an order the tree fixes. A sibling whose type a list takes is typed first. When both are
`list` nodes (the two branches of an `if` with no type of its own, the two sides of `eq` or `ne`), a non-empty one is
typed first and the other takes its type; when both or neither are empty, `then` goes before `else` and `left` before
`right`. A list is typed before its items, except one with no expected list type, which types its first item, takes
its type and gives that `of` to the other items. An expected `List<T>` or `List<T>?` gives `of` `T`. When the first
item's type is unknown, `of` is `Nothing` and the validator reports the item's fault; an empty list with no expected
type has no type. Its time is linear in the nodes.

**R-SYNTH-54** Rejections. A body the tables don't cover is a failed attempt with `VL0401`, citing one rule (R-SYNTH-59
says which, and where). Its words follow "it" in the retry turn and "the body" in `VL0403` (R-SYNTH-59), and the
retry turn adds the hint.

| Rule | Rejects | Words | Hint (retry turn) |
|---|---|---|---|
| `notation-1` | a character R-SYNTH-50 doesn't allow where it stands: non-ASCII outside a Text literal or comment, a tab, a control character, a `\` outside a Text literal (a line continuation), `` ` ``, `$`, `?` | held a character the notation doesn't allow | names are ASCII letters, digits and `_`; indent with spaces; no Markdown fences |
| `notation-2` | indentation that matches no open block, or grows where no block opens | was indented to no open block | indent a block's lines alike, deeper than its `if`, `elif` or `else` line |
| `notation-3` | a malformed literal: an exponent, `_`, a hex, octal or binary number, a leading zero before another digit, `.5` or `5.`; a string prefix such as `f"…"`, triple quotes, an unknown escape, a `\u` escape naming a lone surrogate, an unterminated Text, two Text literals side by side | wrote a number or Text in a form the notation doesn't have | numbers are plain decimals such as `0.05`; Text is joined with `concat`, a Number made Text with `to_text` |
| `notation-4` | a statement outside R-SYNTH-51 (R-SYNTH-57's steps): one starting `for`, or a reserved word not followed by `=` or `.` (`while`, `def`, `class`, `import`, `from`, `try`, `with`, `pass`, `break`, `continue`, `raise`, `assert`, `del`, `global`, `nonlocal`, `yield`, `async`, `await`, `match`, …); `;`; an augmented operator (`+=`); several, unpacked, dotted or annotated targets (`x = y = 1`, `a, b = …`, `x.y = 1`, `x: int = 1`); a bare expression that is not its block's last statement | had a statement the notation doesn't have | by form: `def`, `class`: write only the body, and inputs are read by their names; `import`, `from`: builtins need no import, write `floor(x)`, `reduce(…)`; any other: a loop is a comprehension, `sum`, `all`, `any`, `next`, `sorted` or `reduce`, and a name is assigned once with `=` |
| `notation-5` | a block that doesn't end in a terminal statement (an empty block or branch, a bare expression that is its block's last statement, a header line with anything but `return e` after its colon, and an `if` with no `else` and nothing after it included), or a statement after a terminal one | had a block that doesn't end in `return` | end each block with `return <expression>` |
| `notation-6` | a name bound twice (R-SYNTH-51), or bound in the reserved form `__v<n>` (R-SYNTH-52) | bound a name that was already bound | use a new name |
| `notation-7` | an operator outside R-SYNTH-52: `**`, `%`, `//`, `@`, `&`, `\|`, `^`, `~`, `<<`, `>>`, unary `+`, `:=`, `is` or `is not` with anything but `None` on its right | used an operator the notation doesn't have | `a % b` is `a - b * floor(a / b)`; `a // b` is `floor(a / b)`; `**` is a product or a `reduce` over `range` |
| `notation-8` | an expression form outside R-SYNTH-52 (R-SYNTH-57): a subscript or slice, a dict or set (`{`), a tuple anywhere but as the whole right operand of `in` (`return a, b` and `for a, b in xs` included), a call on anything but a name (`x.f(…)`, `math.floor(x)`), `*` or `**` arguments, a `lambda` or generator elsewhere than R-SYNTH-52 allows, a comprehension with more than one `for` or `if`, a keyword argument to a catalog builtin | used an expression form the notation doesn't have | by form: `{`, a tuple: write a list `[a, b]` (a record is `Type(field=value, …)`); a call on `math.…` or `functools.…`: builtins need no import, write `floor(x)`, `reduce(…)`; a keyword argument: a builtin's arguments are positional; any other inside a call of a name `notation-10` gives a hint: that hint; any other: items are found with `next(…)` and picked with comprehensions |
| `notation-9` | a form of R-SYNTH-52 in the wrong shape: `sorted` with other than one positional list, a keyword other than `key` and `reverse` or one of them twice (either order is fine), a `key` that is not a one-parameter `lambda` or a `reverse` that is not `True` or `False`; `reduce` other than a two-parameter `lambda`, a list and an `init`; `next` other than two arguments whose first is a parenthesised generator with an `if` whose item is its own variable (`next((x for x in xs), None)` and `next((x.a for x in xs if p), None)` included); `all` or `any` with other than one argument; `sum` with a generator and other than one argument, and any of these four with a bare generator beside other arguments (`sum(v for v in xs, 0)`, `next(x for x in xs if c, None)`); `len` or `unwrap_or` with the wrong number of arguments, or any keyword argument to one of these forms but `sorted`; a record with a positional or repeated field | wrote a form in the wrong shape | that form's row: `sorted(xs, key=lambda x: k, reverse=True)`, `reduce(lambda acc, x: e, xs, init)`, `next((x for x in xs if c), None)`, `all(c for x in xs)`, `sum(e for x in xs)`, `len(x)`, `unwrap_or(x, default)`, or `Type(field=value, …)` with each field once |
| `notation-10` | a call of a listed Python built-in that is not a callable built-in of the request's catalog: `max`, `min`, `int`, `float`, `str`, `bool`, `list`, `dict`, `set`, `tuple`, `print`, `enumerate`, `zip`, `map`, `filter`, `pow`, `divmod`, `isinstance`, `type` | called a Python built-in that isn't one of Velme's | by name: `max`, `min`: `max(a, b)` is `a if a >= b else b`, and `maximum(xs)` is `None` for `[]`; `int`: `floor(x)`; `str`: `to_text(x)`; `pow`, `divmod`: as `notation-7`'s; `map`, `filter`, `enumerate`, `zip`: a comprehension; any other: none |
| `notation-11` | a list whose item type R-SYNTH-53 can't tell | had a list whose item type Velme couldn't tell | return the list, put it in a record field, or compare it with a typed value |
| `notation-12` | a body over 64 KiB, or lowering to IR nested deeper than 128 or to more than 10 000 nodes (`compiler/21` §7): both counted on the lowered IR while lowering, so `elif` chains, early returns, comprehensions and chained comparisons count at their lowered depth, and a chain's middle operand twice (its copy counted before it is made, R-SYNTH-52); the parser also stops at forms, brackets or operator chains held in chains of another precedence nested deeper than 128 in the text (R-SYNTH-57), so neither it nor lowering recurses past the cap | was too large or nested too deeply | simplify it, or the plan may be too big for one goal |
| `notation-13` | any other syntax error: an unbalanced bracket, a missing `:`, an unexpected token or end of text | had a syntax error | |

The rules are `notation-1` to `notation-13`. A `None` test on a value that can't be `nothing` is not a notation rule (an
earlier draft's `notation-14`, withdrawn, D-157): it lowers to `eq` like any other (R-SYNTH-52), the validator rejects
it with `types-14`, and velme-synth adds that finding a notation hint (R-SYNTH-59).

**R-SYNTH-55** Parser and lowering. A hand-written module of `velme-synth` (not a crate) with its own fuzz target. It
is a pure function of the body text and the request: deterministic, total (every text gives one IR expression or one
rejection, chosen by R-SYNTH-59), never panics, never recurses past the depth cap, and stops at the node cap, so its time is linear in the body. A rejection is a validation stage of its own, `notation`, before stage 1, so two attempts share a cause when they
break the same `notation-N` rule (R-SYNTH-31, where it belongs to the "reply that isn't a body" row). The retry turn
gives the rule's line and column in the body, its words, its hint and the failing lines (R-SYNTH-56); the learner's
`VL0403` gives the words only and never repeats the body's text (R-SYNTH-22). Lowering keeps, beside the IR and never in it, the span of the notation
each node came from, so a validator finding at a JSON path under `/body` is reported in the retry turn at the line and
column of the innermost node at that path, and R-SYNTH-49's hints name the notation's forms (an input read by its name,
a field as `x.f`) in place of nodes. The grammar version is a constant, bumped with any change to the parser, these
tables or the type rules lowering consults (R-SYNTH-58) that can change the IR or the rejection a body gives, and it enters `input_version` (R-SYNTH-40), so it enters
`synthesis_key` and the replay fixture's name (R-SYNTH-43): a recorded body is never lowered again by another grammar
(D-93, D-97). It is a decimal integer, `1` for the rules of this section, bumped by one. An LLM provider's options
hash (R-SYNTH-40) gains, in `notation` only, the members `"notation_grammar": "<version>"` and `"notation_prompt":
"<hex>"`, the BLAKE3 of `notation.txt`'s bytes as lowercase hex (R-SYNTH-56), so other formats keep their keys and an
edit to `notation.txt` re-keys only `notation` builds; an `external` backend's `input_version` is `0.1+reply_format=notation@1`. After lowering, a body takes the
`ir-json` path unchanged: the canonical-number check (`bad-number`), assembly (R-SYNTH-10) and the validator. The fuzz
target is `notation` (`fuzz/fuzz_targets/notation.rs`, in CI's `FUZZ_TARGETS`, AC-QA-06; the fuzz crate depends on
`velme-synth` with `default-features = false`): any input that is UTF-8 (other input is skipped, since a body is a JSON
string) as the body of each goal of `tests/golden/ir/goals.velme` give a lowered body or a rejection, never a panic; a lowered body is lowered
again to the same canonical IR and then validated, and every pointer `infer_list_types` or the validator reports
resolves to a span. Its corpus, `fuzz/corpus/notation/`, starts from the golden bodies of AC-SYNTH-50 and AC-SYNTH-51
and is replayed by `cargo test` on stable.

**R-SYNTH-59** Order, place and feedback. A body gives one rejection, the first in this phase order, each phase
deciding only its own rules:
1. size: the 64 KiB check (`notation-12`);
2. lexing and parsing: the first error the lexer and parser meet reading from the start (the parser takes tokens as
   the lexer makes them, so it stops at whichever error comes first in the text, R-SYNTH-50, R-SYNTH-57). This phase
   decides `notation-1` to `-5`, `-7` (`is` with anything but the token `None` on its right included), `-8` but for a
   keyword argument to a catalog builtin, `-9` for the forms its callee token tells (`len`, `unwrap_or`, `sum`, `all`,
   `any`, `next`, `sorted`, `reduce`), `-12` for nesting, and `-13`;
3. lowering, for a body that parses: the first error in lowering order (R-SYNTH-52) among the rules that need the
   request: `notation-6`, `-8` for a keyword argument to a catalog builtin, `-9` for a record, `-10`, and `-12` for
   the IR caps;
4. typing: the `infer_list_types` finding R-SYNTH-58 picks, `notation-11`.

A span is a byte range of the body. A rule's span is: for `notation-1` the character; `-2` the line's indentation;
`-3` the literal (an unterminated Text from its quote to the line's end); `-4` the token R-SYNTH-57 decides it by
(`for` or the reserved word, `;`, the augmented operator, the second `=`, the `,` or `:` after a first `NAME`), or the
bare expression; `-5` the bare expression, the token after a header's colon, the statement after a terminal one, or
else the header token (`if`, `elif`, `else`) of the block that lacks its `return`, and for the top level, which has no
header, the end of the text (an empty body included); `-6` the binding name; `-7` the operator; `-8` the token
R-SYNTH-57 names, or a keyword argument's name; `-9` the call, from its name to its `)`; `-10` the called name; `-11`
the list; `-12` the body's start, whatever the cause; `-13` the unexpected token, or at the end of the text the last
line's end. A place is shown as `line L, column C`, both from 1, at the span's start, the column counting Unicode
scalar values from the line's start. A validator finding's place is that of the innermost lowered node at or above its
JSON pointer (R-SYNTH-52's spans).

In the next request (§3.1), the diagnostic of a notation rejection, and of a validator finding with a place, carries
`at` (`{"line": L, "column": C}`, both integers of at least 1, no other members) and `quote` (the body lines its span
covers, at most 3, each cut to 160 Unicode scalar values; in the request schema `maxItems` 3 and `maxLength` 160), and
no `path`; a finding with no place keeps its `path`. Both members are optional and absent outside `notation`, so other
requests keep their bytes (D-156's rule for `request_version` `0.1`). Only the latest attempt's diagnostics carry them:
the primary diagnostics `retry_history = "latest"` carries from earlier attempts (R-SYNTH-37) keep neither `at` nor
`quote`, since their bodies are not resent, nor a `path` they didn't have. The retry turn renders a diagnostic with
`at` as `- VL0401 at line 3, column 9: <message>`, then each quoted line as `  | <line>`, then the detail with the
hint. A notation rejection's message is ``Velme couldn't read the body: it <words> (notation-N).``; its `VL0403` line
and `--verbose` entry say ``the body <words> (notation-N)``, with no place and no quote (R-SYNTH-22). A `body` that is
not a string is ``the reply had a body that isn't a string``, log cause `body-not-string` (R-SYNTH-10, R-SYNTH-23).

In `notation`, R-SYNTH-49's hints name the notation's forms, and velme-synth replaces or adds the hints below. Each is
chosen from the finding's rule id, the validator's subject and the notation form the node at the finding's pointer was
lowered from, which lowering records beside its span; never from a message's text:
- `names-7`, `names-8`: an input or call result is read by its name, a field as `x.f`;
- `names-11` on an operator name: its notation (`add` is `a + b`, `not` is `not a`, `is_empty` is `x is None` for an
  optional and `len(x) == 0` for a list or Text); on the goal name of one of a composite's calls: ``that goal is
  already called: its result is the name `<binding>` ``;
- `types-12` (the validator's help replaced): ``Velme doesn't narrow after `x is None`: read it as `unwrap_or(x, d).f` ``;
- `types-14` on an `eq` or `ne` lowered from a `None` test (R-SYNTH-52): ``only an optional `T?` can be `None`; test a
  list or Text with `len(x) == 0` ``;
- `types-15` on `add`, `sub`, `mul` or `div` with a Text operand: `+ - * /` are for Numbers only and Text is joined
  with `concat`; on an `or`: ``a default is `unwrap_or(x, d)`, not `x or d` ``;
- `types-26` on `round` given two arguments: ``to cents, `round(x, 2)` is `round(x * 100) / 100` ``.

**R-SYNTH-56** Token cost. The notation exists to cut tokens (D-152), so its request is held to a budget:
- no IR JSON schema is sent: `write_goal`'s input and `ollama`'s `format` are the `{"body": string}` reply schema (or
  the question object), and `schema_in_prompt = "full"` adds only that schema;
- the prompt's notation reference is a summary of the R-SYNTH-51 and R-SYNTH-52 tables, one line per group of forms,
  plus the lines saying where the notation differs from Python (exact decimals, `/`, `round`, no narrowing after
  `x is None`, no truthiness); never the grammar of R-SYNTH-50 or the rejection table of R-SYNTH-54. The IR node lines
  (R-SYNTH-35) and the rules spelled as nodes are left out, and the worked examples (D-104) are written in the notation.
  That text is `crates/velme-synth/prompts/notation.txt`, hashed into the options member `notation_prompt`
  (R-SYNTH-55), not into `prompt_version`, so an edit to it re-keys only `notation` builds (D-157, a narrow exception
  to D-97). It has three parts, split at the lines `--- composite task ---` and `--- retry ---`. In `notation` the
  first replaces the template from `Output contract` through the worked examples; the second replaces the composite
  template's task paragraph (its lines from `Task:` up to the `Goal:` line); the third replaces the retry turn's last
  line (`Return the corrected …`). The allowed builtins leave out the collection primitives (forms of the
  notation);
- the fixed prefix (R-SYNTH-34: the `anthropic` system prompt and tool definitions, then the prompt through the
  allowed builtins) stays within about 1.5k tokens, checked in bytes by AC-SYNTH-55;
- the feedback for a notation rejection, or for a validator finding with a span, quotes only the body lines that span
  covers (at most 3, each cut at 160 Unicode scalar values) beside the rule, never the whole body again; the carried reply itself
  is history, sent as received (R-SYNTH-37, R-SYNTH-44). The quote goes only to the model that wrote it, never into a
  diagnostic, the log or `--json` (R-SYNTH-22).

*Informative:* the first part of `notation.txt` is 2,611 bytes (the whole file 2,865), of which the notation reference
is 1,443 and the worked examples 555, about 600 tokens together at about 3.3 bytes a token. It replaces 4,373 bytes of
the `ir-json` leaf prompt (the output contract, the 20 node lines and the JSON examples), and the collection
primitives' 629 bytes of builtin lines go too, so the leaf golden goal's prompt prefix falls from 5,874 bytes to about
3,500, and the tool's reply schema from about 10 KB to under 0.5 KB. With the system prompt and tool definitions the
notation's fixed prefix is about 1.1–1.2k tokens, just above the 1,024-token minimum a prompt cache needs, so a later
trim can drop it below and lose caching: P0e compares arms in billed-equivalent tokens, from the cache counts of the
synth log v2 (R-SYNTH-23, D-155), not in raw input tokens. The 10 loan goals of AC-SYNTH-50 are 2,547 bytes of
notation, 2,808 as `{"body"}` replies, against 13,319 bytes of canonical `ir-json` replies: the output is 4.7 times
smaller.

*Informative, the implementation's shape* (names are the implementer's to keep or change; behaviour is the rules'):
`velme_synth::notation`, a module (`src/notation/`: `lex`, `parse`, `lower`), exports `GRAMMAR_VERSION: &str = "1"`
and `lower(text: &str, request: &SynthRequest, ir: &velme_ir::Request<'_>) -> Result<Lowered, Rejected>`, where
`Lowered { body: velme_ir::Node, spans }` answers `spans.at(pointer) -> Option<Span>` (R-SYNTH-59),
`Rejected { rule: Rule, span: Span, hint: Option<String> }`, `Rule` is the closed set `notation-1..13` with `id()`,
`words()` and its hint (fixed, or chosen by form for `notation-4`, `-8` and `-10`), and `Span { start, end }` is a byte range; `locate(text, offset) -> (line, column)` and
`quote(text, span) -> Vec<String>` serve R-SYNTH-59. `velme_ir::infer_list_types(body: &mut Node, request: &Request<'_>)
-> Vec<pointer>`, the untyped lists, is R-SYNTH-58, built on the per-kind typing functions of `validate.rs`'s stage 4;
the notation's policies (which forms exist, `None` tests, hints) stay in `velme-synth`, so `velme-ir` gains no notation
type. The fuzz crate depends on `velme-synth` with `default-features = false`.
`ReplyFormat::Notation` is named `notation` (`from_name`, `name()`, the request schema's enum) and its reply schema is
`{"anyOf": [{"type": "object", "properties": {"body": {"type": "string"}}, "required": ["body"]}, <question>]}`, as
`compact`'s (R-SYNTH-36). `AttemptDiagnostic` gains `at: Option<{line, column}>` and `quote: Vec<String>`, both
skipped when empty. No `MalformedReason` is added: a notation fault is a reply's, not a response's (R-SYNTH-07).

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
by the template's retry text (D-95); `anthropic` sends a carried reply as the tool call it was and the diagnostics as
that call's result (R-SYNTH-44, D-150).
At most `max_retries` (default and cap: 3) retries follow the first attempt.
**R-SYNTH-12** Transport errors (`RateLimited`, `Unavailable`, `Timeout`) are retried up to 2 times per attempt and do
not consume a synthesis retry, except that the LLM providers (`anthropic`, `ollama`) never retry a `Timeout` that struck
after the request was sent: a generation that ran out of time would only run out again, so the attempt ends with
`VL0404` at once (D-110). An operating-system `TimedOut` error counts as such a timeout: the client's own connect limit
fires long before the operating system's, so it strikes mid-exchange (D-129). The waits are 1 s then 2 s with no jitter;
a `RateLimited` with `retry_after` waits that long instead, at most 30 s. The sleep is injected, so tests take no wall
time (D-95).
**R-SYNTH-13** After the last retry the goal fails with `VL0403`, whose message states the cause (R-SYNTH-31);
`--verbose` adds every attempt's diagnostics. The source `plan` is never modified; nothing is written to the artifact
store or the lock.
**R-SYNTH-31** The learner never needs `--verbose` to know what to fix. Each failed attempt has one primary diagnostic
(R-SYNTH-15 for verification, otherwise the first in R-CMP-16 order). Two attempts share a cause when that diagnostic
has the same code and names the same check, example or validator rule (same stage and rule name, whatever the JSON
path, D-95), whatever the input; every `Malformed` reply is one cause (R-SYNTH-07). `VL0403` reports the cause
shared by the most attempts, ties going to the later attempt, says how many attempts it covers, and takes its details
(input, values) from the latest attempt with that cause. Each other cause is one note, in first-seen order.

| Cause | Message names | Help suggests |
|---|---|---|
| `VL0502` | the example: given, got, expected | check the example, or say in the plan how that case is handled |
| `VL0501` / `VL0503` | the check and its counterexample (R-SYNTH-19) | say in the plan what happens for that input, or narrow the check with `if … then …` (D-6) |
| `VL0401` / `VL0402`, a built-in that doesn't exist or is called wrongly (`names-11`, `types-6`, `types-7`, `types-25`, `types-26`) | the validator rule that kept breaking | the plan may need more than the listed builtins can do: simplify it or split the goal |
| `VL0401` / `VL0402`, a body too large (`structure-1`, `structure-3`, `resources-*`) | the validator rule that kept breaking | the plan may be too big for one goal: simplify it or split the goal (D-103) |
| `VL0401` / `VL0402`, a reply that isn't a body (`schema-1`, or no `body`, or not JSON, a `Malformed` reason, a `notation-N` rule, R-SYNTH-07, R-SYNTH-54) | what was wrong with the reply, in Velme's words | the AI helper's reply wasn't in a form Velme can use: build again, or try another model (D-103) |
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
**R-SYNTH-33** The question is untrusted text (R-SYNTH-22). Control and format (`Cf`) characters other than bidi
controls, newlines and ANSI escapes included, become spaces and whitespace runs collapse to one; bidi controls are
kept and escaped when shown (R-CLI-17, D-108). The result must then be 1..=280 Unicode scalar values, or the reply
is a failed attempt with `VL0401`. It is shown only as a note, quoted and labelled as the AI helper's question, and as
a plain string in `--json`.

**R-SYNTH-49** The diagnostic a retry carries for a name or field the goal doesn't have says what is there and which
node reads it, from the request alone, with the node named as the reply is asked to spell it (the alias too in the
`compact` format, R-SYNTH-36; the notation's forms in `notation`, R-SYNTH-55). For `names-7` and `names-8` it lists the goal's inputs with their types (each read with
an `input` node) and, for a composite, the call bindings (each read with a `local` node); for a dotted name it says that
a field is read with a `field` node over the `input` node, or over the `local` node when the name starts at a call
binding; for `names-9` and `names-10` it lists the fields of the record named; for `types-15` on `add`, `sub`, `mul` or
`div` with a Text operand, that these are for Numbers and Text is joined with `concat`, and for no other operator or
operand, nothing; for a `names-11` name that is an operator of the schema, that it is written as a `binary` (or `unary`)
node with that `op`, and for any other name, the built-ins that the request's catalog lets a `builtin` node
call (not the collection primitives). The validator reports what a finding is about as data, not text, so the hint never
depends on parsing its message. Each distinct hint is sent once per attempt, on the first finding that has it. Nothing
of the reply's own text is repeated in a hint (R-SYNTH-22) (the failing lines a notation retry quotes, R-SYNTH-56, are not a hint) (D-104).

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
model, `max_retries` 0..=3 per R-SYNTH-11, timeouts, output tokens, `max_calls_per_build`). `max_output_tokens` defaults per provider: 8192 for `anthropic` and 2048 for `ollama`, whose replies are a body only (D-103, D-110). The service URLs and the user's ceilings are read from the user-level config, not the project (`tooling/40` R-CLI-13, D-105).

**R-SYNTH-20** API keys come only from the environment (`VELME_API_KEY`, or the `ANTHROPIC_API_KEY` fallback;
`tooling/40` §5.2). A key-like value in `velme.toml` is an error. Keys never appear in logs, traces, fixtures,
artifacts, diagnostics or `--verbose` output; the replay recorder stores no headers.
**R-SYNTH-21** `max_calls_per_build` bounds cost: when reached, remaining goals whose children all built fail with
`VL0403` and a message naming the limit; the rest get `VL0409` (R-SYNTH-42). A **provider call** is one attempt at a
candidate, whatever its reply, a `{"pending"}` or `{"question"}` included (D-56). Transport retries (R-SYNTH-12), the
Ollama digest lookup and `describe` are not calls and never count toward the cap, but they are provider contact: a
build that must make none, such as a fully cached one (D-57), makes none of them either (D-92). Each `complete()`
invocation is exactly one call, even when every transport try inside it failed and it ended in `VL0404`. The build summary reports calls, tokens in/out, prompt-cache tokens read and written
(R-SYNTH-34) and store/lock hits; the tokens a refused or cut-off response billed count, as a reply's do (D-155). A lock hit is verified again on every build, leaves included, on the interpreter with
no provider call; one that no longer passes is stale and synthesized in the ordinary way (runtime/32 R-ART-22, D-100).

## 9. Untrusted plan text

**R-SYNTH-22** Plan, check and example text are untrusted data: they are fenced in the prompt and the instructions
state they describe intent only. Velme does not rely on this for safety — whatever the LLM returns must still pass
validation and runs sandboxed with no capabilities (INV-1, INV-4, tooling/41).

## 10. Local synthesis log

**R-SYNTH-23** Each provider request appends one JSON line to `.velme/synth-log.jsonl`, for every provider and never for a
lock or store hit (D-109, D-154, D-155). A request here is one `complete()` call (R-SYNTH-21): transport retries inside it add no line, and
its latency covers them. The line is one object, keys in this order, no spaces:
`{"format":"velme-synth-log/2","time":"2026-10-03T12:00:00.000Z","file":"player.velme","goal":"CalculateScore","key":"b3:…","provider":"anthropic","model":"claude-sonnet-5-5","attempt":1,"outcome":"VL0402","cause":"types-15","stop_reason":"tool_use","reply_shape":{"blocks":["text","tool_use"],"keys":["body"]},"tokens_in":812,"tokens_out":143,"cache_read_tokens":4210,"cache_write_tokens":0,"latency_ms":2041,"reply_bytes":386,"reply_format":"ir-json"}`.
`time` is UTC with milliseconds, and the clock is injected so tests are exact. `file` is the project-relative path (R-CLI-19),
`key` the synthesis key, `attempt` counts from 0, `outcome` is `ok` or the `VL` code the attempt ended with, and the token
counts are `null` when the provider reports none: a count it doesn't report, or reports as 0, is written as `null`.
`cause` is `null` exactly when `outcome` is `ok`, else the attempt's cause (R-SYNTH-31) as one token of a closed list,
never text from a reply or a provider (D-155):
- a validator or notation rule id (`types-15`, `notation-7`), or `schema-1`;
- for a reply that isn't a body, `not-json`, `no-body`, `body-and-question`, `bad-question` (empty, too long or not
  text), `unknown-alias` (compact), `body-not-string` (notation) or `bad-number` (a number canonical JSON refuses);
- `question` (R-SYNTH-32);
- `malformed-<reason>` with the R-SYNTH-07 reason id (`too-large`, `response-not-json`, `cut-off`, `not-one-tool`,
  `unknown-tool`, `no-message`, `reply-not-json`, `reply-not-object`), or `malformed` when there is none (replayed or
  scripted);
- `check-<n>` or `example-<n>`, 1-based in the goal's source order;
- `runtime-<code>`, for a run of the candidate that failed, on an example or a generated input, before its example or
  checks were judged (`runtime-VL0601`; `runtime-VL0603` when the watchdog stopped it);
- `internal`, for a bug of Velme's, not of the reply (`VL0607`);
- a `ProviderError` variant as fixtures name it (R-SYNTH-43): `not_configured`, `key_rejected`, `token_rejected`,
  `token_required`, `rejected`, `unavailable`, `rate_limited`, `refused`, `timeout`, `backend_failed`, `pending`, `file`,
  `internal`.

`stop_reason` is the provider's own (`anthropic` `stop_reason`, `ollama` `done_reason`) when it is 1..=32
characters of `a`–`z` and `_`, `"…"` for any other value, and `null` when there is none (`external`, `replay`,
`scripted`). `reply_shape` is `null` when no response came back (`cause` a `ProviderError` variant other than `refused`)
or the response could not be read (`malformed-too-large`, `malformed-response-not-json`). A response that was read but
held no reply (`refused`, `malformed-cut-off`, `malformed-not-one-tool`, …) keeps its `stop_reason`, its token counts and
its `blocks` (`null` for a provider without blocks), with `keys` `[]`. Else `blocks` lists the kinds of the response's content
blocks in order (`text`, `tool_use`, `thinking`, `redacted_thinking`; any other kind as `"…"`), at most 64 of them
followed by one `"…"` if there were more, or is `null` for a
provider without blocks, and `keys` lists the reply object's top-level keys that are an IR property name, a compact
alias of one, `body`, `question`, `pending` or `error`, sorted and without repeats, then one `"…"` if any other key was there
(`[]` when the reply is not a JSON object). `reply_bytes` is the byte length of the reply as received (for `external`, the canonical reply document Velme reads, §3.2), `null` when no reply
came back, and `reply_format` the reply format the request used by its `[synthesis]` name (`ir-json`, `compact`, `notation`). `tokens_in` is uncached input; `cache_read_tokens` and
`cache_write_tokens` are what the provider reports as read from and written to its prompt cache, `0` included (a
cache miss is information), and `null` when the provider doesn't report cache use at all (`ollama`, `external`,
`replay`, `scripted`), an exception to the token-count rule above. A file may hold lines of both formats; each line names its own. Within
`velme-synth-log/2` Velme may add members after the last one, add tokens to the `cause` list and add names to the `blocks`
and `keys` sets. A reader ignores members it doesn't know and treats an unknown cause token as opaque. Removing, renaming
or reordering a member, or changing its type or when it is `null`, takes a new format number (D-155). Plan text, prompts, replies (beyond
`reply_shape`) and values are never written, and there is no
opt-in for them in v0.1. The runtime crate writes the file, refusing a link and anything that is not a regular file as the
artifact store does (`runtime/32` R-ART-09); a write that fails is a `-v` notice, never a build failure. When Velme creates
`.velme/` it also writes `.velme/.gitignore` naming `synth-log.jsonl`, never overwriting an existing one. The log never
leaves the machine (tooling/41).

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
| AC-SYNTH-08 | `--locked` with a stale entry fails with `VL0702`; `--offline` with a goal that isn't fresh in the lock fails with `VL0404` in the offline wording, and both together fail with `VL0702`; none constructs a provider or contacts one. |
| AC-SYNTH-09 | No API key string appears in any output, log, fixture or artifact after a recorded build (grep test with a sentinel key). |
| AC-SYNTH-10 | `replay` provider reproduces a recorded build byte-for-byte (same artifacts, same lock). |
| AC-SYNTH-11 | Against a mock Ollama server, the `ollama` provider sends the reply JSON Schema as `format` with temperature 0 and no streaming, and an accepted artifact records `"provider": "ollama"` and `<model>@<digest>` as `model_version`. |
| AC-SYNTH-12 | Changing the digest behind the same Ollama tag, with an up-to-date lock, makes zero requests; after deleting the store entry, the next build misses on the new `synthesis_key`. |
| AC-SYNTH-13 | An unreachable Ollama server yields `VL0404`; a model the server doesn't have yields `VL0405` with an `ollama pull` hint. |
| AC-SYNTH-14 | The `external` `synthesize` request body for a golden goal matches its snapshot, validates against the committed request schema, and contains no file paths, environment values or API keys. |
| AC-SYNTH-15 | An `external` backend returning a hostile body (a `call` node, an unknown builtin) is rejected with `VL0402` in both cases (D-63); nothing is stored. |
| AC-SYNTH-16 | A reply that is not JSON, a body over the cap, a `5xx` after the transport retries, a redirect and an `{"error"}` reply each fail with `VL0406` naming the goal and backend, with the tail of the response body as a note; the lock is unchanged and no synthesis retry is made. A refused connection or a timeout is `VL0404`; a `401` or `403` is `VL0405` naming `VELME_EXTERNAL_TOKEN`. |
| AC-SYNTH-17 | With `max_retries = 1`, a second `external` request carries the first reply and its diagnostics in `attempts`; with the default 0, a rejected reply fails the goal after one request. |
| AC-SYNTH-18 | With sentinel values in `VELME_API_KEY` and `ANTHROPIC_API_KEY`, the `external` service receives neither and neither appears in any output or file; the `VELME_EXTERNAL_TOKEN` token is sent only as `Authorization: Bearer` and appears in no output, fixture or `replay.json`; an `external_url` named in the project `velme.toml` is `VL0902`. |
| AC-SYNTH-19 | A fully cached build (every goal fresh in the lock) with `--provider ollama` or `--provider external` sends no request to the server, and resolves no model digest / runs no `describe` (D-57). |
| AC-SYNTH-20 | With `stop_on_repeat = false`, four scripted replies where attempts 1, 2 and 4 fail the same check on different inputs and attempt 3 fails an example: `VL0403` names the check with attempt 4's counterexample and "3 of 4", and one note names the example. A sentinel string in the replies' text appears nowhere in the output. |
| AC-SYNTH-21 | Four scripted replies failing two causes twice each, alternating: `VL0403` reports attempt 4's cause. |
| AC-SYNTH-22 | A scripted `{"question"}` reply with `max_retries = 3` fails the goal with `VL0407` showing the question after exactly one provider call; nothing is stored, the lock is unchanged, and another goal in the same file still builds. |
| AC-SYNTH-23 | A question containing a newline and an ANSI escape is shown with both replaced by spaces; an empty question and a 281-character question are each a failed attempt with `VL0401`. |
| AC-SYNTH-24 | An `external` backend replying `{"question"}` fails the goal with `VL0407`. |
| AC-SYNTH-25 | The rendered prompts for two different golden goals share a byte-identical prefix up to the end of the allowed builtins; the mocked `anthropic` request marks a cache breakpoint there, and with `prompt_cache = false` marks none. Both give the same `synthesis_key`. |
| AC-SYNTH-26 | A scripted `compact` reply expands to IR whose artifact's `ir` member is byte-identical to that of the same IR sent as `ir-json` (the manifests differ in `synthesis_key` and `prompt_version`, `runtime/32` R-ART-25); a reply with an unknown alias is a failed attempt with `VL0401`. |
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
| AC-SYNTH-44 | A build with a scripted provider appends exactly one line per `complete()` call to `.velme/synth-log.jsonl`, none for a lock or store hit, each with the R-SYNTH-23 keys in order and an injected time; the line holds no plan text, prompt or reply; a log path that is a link, or a write that fails, gives a `-v` notice and the build still succeeds; `.velme/.gitignore` is created with the directory and never overwritten. |
| AC-SYNTH-45 | The request an `ollama` provider sends carries `max_output_tokens` 2048 and an `anthropic` one 8192 unless configured, and a `Timeout` after the request was sent is not retried by either (one request, `VL0404`), while a refused connection still is (R-SYNTH-12, D-110). |
| AC-SYNTH-46 | The request an `anthropic` provider sends has `tool_choice` `auto`, a system prompt asking for exactly one of the two tools and no `temperature`; in a retry each earlier reply is a `tool_use` block whose id the next user turn's `tool_result` names; an unnamed `4xx` (a 400) is `VL0405` with the status and the API's message, cleaned, with the key replaced by `***`, and quoted, not retried, and the next goal still sends its request (R-SYNTH-07, R-SYNTH-44, R-SYNTH-45, D-150). |
| AC-SYNTH-47 | In `compact`, the rendered output contract and the reply schema spell the body key by its alias; a scripted reply under that alias builds an artifact whose `ir` member is byte-identical to that of the same body sent as `ir-json` (AC-SYNTH-26), and a compact reply keyed `body` is a failed attempt with `VL0401`; the next retry turn carries the earlier reply byte-identical to the one received (R-SYNTH-36, D-153). |
| AC-SYNTH-48 | The generated alias table has no two equal aliases and no alias equal to a canonical property name or `kind` tag; the prompt rendered in each reply format says neither "reply with JSON" nor to call a tool; a mocked `anthropic` reply with only a text block is a failed attempt with `VL0401` whose message, `VL0403` cause and log `cause` (`malformed-not-one-tool`) carry the `Malformed` reason, and two `Malformed` replies with different reasons share one cause, so `stop_on_repeat` stops after the second, and a sentinel in the text block appears in no output or log (R-SYNTH-07, R-SYNTH-36, D-153). |
| AC-SYNTH-49 | A build against a mocked `anthropic` writes `velme-synth-log/2` lines with the R-SYNTH-23 keys in order: `cause` `null` on `ok`, the rule id for a validator failure and `check-1` for a check failure; `stop_reason`, `reply_shape` and the cache token counts from the mocked reply and usage; a reply key and a block kind outside the R-SYNTH-23 sets written as `"…"`. With `scripted`, `stop_reason`, `blocks` and the cache counts are `null`; a mocked `anthropic` usage with `cache_read_input_tokens: 0` writes `0`. A sentinel used as a reply key, in a text block and in the body appears nowhere in the log (D-154). A sentinel in an unknown tool's name, an external `pending` text, a `backend_failed` body or a `rejected` message appears nowhere in it either. A refusal and a cut-off reply, also through `--record`, keep `stop_reason` and `blocks`, a cut-off's `tokens_out` is logged and counted in the build summary, more than 64 blocks are listed as 64 and `"…"`, and `reply_bytes` and `reply_format` are written (D-155). |
| AC-SYNTH-50 | Each row of R-SYNTH-51 and R-SYNTH-52 has a golden notation body that lowers to its expected canonical IR, stored beside it and compared by the test (this one and every golden accepted body of AC-SYNTH-58), whose failure message says a changed lowering needs `GRAMMAR_VERSION` bumped (R-SYNTH-55); `x is None`, `None == x` and `x is not None` lower to `eq` and `ne` of `x` and `nothing`; `any(xs) and all(ys)` names their parameters `__v0` and `__v1`, or `__v1` and `__v2` for a goal with an input named `__v0`. The 10 goals of the loan example, written by hand in notation (`crates/velme-synth/tests/notation/loan/`: the program `loan.velme`, each goal's body `<Goal>.notation` and its `ir-json` reply `<Goal>.json`), lower, validate and pass their examples, and each gives an artifact whose `ir` member is byte-identical to that of its `<Goal>.json` sent as `ir-json` (AC-SYNTH-26). In `notation`, `write_goal`'s input schema and `ollama`'s `format` take `{"body": string}` (or the question object), and `--emit ir` shows IR (D-152). |
| AC-SYNTH-51 | Each `notation-N` rule of R-SYNTH-54 has a golden rejected body, each a failed attempt with `VL0401` citing that rule; the retry turn gives its line, column, words and hint and quotes only the failing line, and `VL0403` its words alone; the hint is the per-form one where R-SYNTH-54 gives several (`def` for `notation-4`, a tuple and `math.floor(x)` for `notation-8`). A sentinel on another line of a rejected body appears in no retry feedback, diagnostic, output or log (R-SYNTH-56, D-152). |
| AC-SYNTH-52 | `[]` takes its item type from each R-SYNTH-53 context in turn (the output type, the other `if` branch, an `==` side, a record field, a catalog parameter, `unwrap_or`'s default, `reduce`'s `init`), and `xs = []` with no context and `len([])` (a generic parameter) are `notation-11` (D-152). |
| AC-SYNTH-53 | The notation parser's fuzz target `notation` runs clean (R-SYNTH-55); a body whose lowered IR nests 129 deep through an `elif` chain written flat, one nesting 129 brackets, one over 64 KiB and nested chained comparisons that would lower past 10 000 nodes are `notation-12`, with no stack overflow; the same body lowered twice gives byte-identical IR. A validator finding on a lowered node (`names-8` on an unknown name, `types-15` on `+` with Text) is reported in the retry turn at the body's line and column, not a JSON path, and R-SYNTH-59's notation hints are given: `x is None` on a Number input (`types-14`), a field read through an optional (`types-12`), `x or 0` on a Number? (`types-15`), `round(x, 2)` (`types-26`) and a composite calling its child goal by name (`names-11`, naming the binding). The depth tests run in a debug build on a thread with a 2 MiB stack (R-SYNTH-55, D-152). |
| AC-SYNTH-54 | Changing the notation's grammar version, or the bytes of `notation.txt`, changes the `synthesis_key` of a `notation` build, not of an `ir-json` one (whose `prompt_version` is unchanged) and no `contract_key`, and a build with an up-to-date lock makes zero calls; a recorded `notation` build replays byte-for-byte, with `reply_format` in `replay.json` (R-SYNTH-40, R-SYNTH-43, D-152). |
| AC-SYNTH-55 | For the leaf golden goal of AC-SYNTH-07 in `notation` with default options, the `anthropic` request body is at most 7,168 bytes and its fixed prefix (system prompt, tool definitions and the prompt through the allowed builtins) at most 5,120 bytes, about 1.5k tokens. Chosen from the same goal's `ir-json` request today: a 6,698-byte prompt (5,874 of it the fixed prefix) plus a reply schema of about 10 KB (about 3k tokens) in the tool definition; 7 KiB is that request without the schema, with about 0.5 KB for system prompt, tools and JSON framing, and the prefix bound makes the notation reference and its examples smaller than the node lines and JSON examples they replace (R-SYNTH-56, D-152). |
| AC-SYNTH-56 | In `notation`, the `anthropic` and `ollama` request bodies hold no IR schema: no `$defs` and no definition name of the IR schema, and `write_goal`'s input schema and `ollama`'s `format` are the `{"body": string}` reply schema, with either `schema_in_prompt` (R-SYNTH-56, D-152). |
| AC-SYNTH-57 | With `reply_format = "compact"`, the `external` request names the format and a compact reply builds an artifact; a default build's request has no `reply_format` member; a compact build whose backend answers under `body` is a failed attempt (`VL0401`, then `VL0403` naming the alias), not `VL0406`; the request's `output_schema` is the compact reply schema (no IR `$defs`); the format enters the external `input_version` (`<request_version>+reply_format=compact@<version>`, the version a 12-digit hash of the alias table), re-keying the build; and a backend whose `describe` lacks `reply_formats` (or does not list `compact`) ends the goal with `VL0406` naming the setting before any `synthesize` request (R-SYNTH-26, R-SYNTH-27, R-SYNTH-36, R-SYNTH-40, D-153, D-156). |
| AC-SYNTH-58 | Golden accepted bodies cover R-SYNTH-50 and R-SYNTH-57: `\r\n` line ends, a comment-only line, a top level indented 4 spaces, a call continued over lines with a trailing comma, a single-line `return` branch, an `elif` chain with no `else` before the rest of its block, `a if c else b if d else e` grouping to the right and `a - b - c` to the left, `-0.5` and `- 0.5` lowering to one literal, `--2` to `neg` of `-2` and `-(2)` to `neg` of `2`, keyword names read as names (`bond.yield`, `T(from=x)` for a record field `from`, an input named `class`, `x.if`), `while` used as a name lowering to `local` (`names-8` from the validator), and `x in (1, 2)` lowering as `x in [1, 2]`. Golden rejected bodies split statements as R-SYNTH-57 says: `match x:`, `while c:`, `x.y = 1`, `x = y = 1`, `x += 1`, `a, b = c` and a bare expression before the block's last statement are `notation-4`, a bare expression last in its block and `if c: x = 1` `notation-5`, `else:` with no `if` `notation-13`, `x[0]`, `(a, b)`, `return a, b`, `[x for a, b in xs]` and `(f)(x)` `notation-8`, and `next((x for x in xs), None)` and `sorted(xs, key=…, key=…)` `notation-9`, while `sorted(xs, reverse=True, key=lambda x: x.a)` lowers (D-157). |
| AC-SYNTH-59 | `infer_list_types`, in `velme-ir`'s tests over hand-built nodes, sets each list's `of` from each row of the R-SYNTH-58 table whatever `of` it was given, the second of two list branches from the first; an item of a typed list takes its `of` (`[[], [1]]` as a `List<List<Number>>` output), and of two list branches the non-empty one is typed first (`[] if c else [1]` with no expected type); it returns only the pointer of an untyped list for `[]` as a `let` binding's value, and nothing for a body whose only fault is an unknown name; a list whose first item is an unknown name gets `of` `Nothing`, and the build's primary diagnostic is that name's `names-8`, not `notation-11`; it visits each node of a 10 000-node body once (D-157). |
| AC-SYNTH-60 | `ReplyFormat::from_name("notation")` is the notation format; the committed `velme-synth-request` schema lists `notation` and the optional `at` (`line` and `column` integers of at least 1, no other members) and `quote` (at most 3 strings of at most 160 characters) members, which a diagnostic carried from an attempt before the latest never holds, and which an `ir-json` or `compact` retry request never holds, so its bytes are unchanged; an `external` `notation` build's `input_version` is `0.1+reply_format=notation@1` and a backend whose `describe` doesn't list `notation` ends the goal with `VL0406`; a `notation` reply whose `body` is an object is a failed attempt with `VL0401` and log cause `body-not-string`, and a notation reply's log line has `reply_format` `notation` and `keys` `["body"]` (R-SYNTH-23, R-SYNTH-55, R-SYNTH-59, D-156, D-157). |
| AC-SYNTH-61 | A body with a syntax error before a lexical one is rejected for the syntax error; one with a `notation-6` before a `notation-13` for the `notation-13`; one with two untyped lists for the first. Each golden rejected body of AC-SYNTH-51 is placed at the line and column R-SYNTH-59 gives, a `%` after a Text holding non-ASCII counted one column per scalar value; a validator finding on a chain's added `and` is placed at the chain's start. The caps are exact: a 65 536-byte body passes the size check and a 65 537-byte one is `notation-12`, as are IR 129 deep and 10 001 nodes, while 128 deep and 10 000 nodes lower; every `notation-12` is placed at line 1, column 1; a body ending in an assignment is `notation-5` at the end of the text, and an untyped `[]` as a chain's middle operand (`a == [] == b`, both sides untyped) is reported once, at its own place, from the first of its two equal spans in a pre-order walk (R-SYNTH-59, D-157). |
