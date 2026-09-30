# 40 — Command-Line Interface

**Status:** v0.1 · **Area:** CLI
**Read when:** adding or changing a `velme` command, flag, config key, environment variable, exit code or output format.
**Depends on:** [SPEC](../SPEC.md), [11-types](../language/11-types.md) (JSON mapping), [30-execution-vibevm](../runtime/30-execution-vibevm.md), [32-artifacts-cache](../runtime/32-artifacts-cache.md), [90-errors-glossary](../reference/90-errors-glossary.md)

## 1. Purpose & boundaries

Defines the `velme` binary: its commands, flags, configuration, input/output formats and how results and failures are
shown. The CLI is a thin shell over library APIs (INV-9): it parses arguments, loads config, calls `velme-sema`,
`velme-synth` and `velme-runtime`, and renders. It owns no language semantics.

**R-CLI-01** `velme-cli` contains no parsing, typing, synthesis or execution logic; every command is a call into a core
crate that the IDE, playground or a build server could make identically.
**R-CLI-02** Every command works offline when it needs no synthesis (`check`, `explain`, `artifact`, and `run`/`test`
against a current lock) — INV-7.

## 2. Commands

| Command | Does | Synthesizes? | Writes |
|---|---|---|---|
| `velme check FILE` | parse, name/type check, call-graph check, validate locked IR if present | never | nothing |
| `velme build FILE` | `check`, then synthesize + verify every goal whose lock entry is missing or stale | yes, stale goals only | `.velme/artifacts/`, `velme.lock` |
| `velme run FILE --goal G [input]` | execute `G` with the locked artifacts; a stale/missing goal is `VL0702`/`VL0701` with a hint to run `velme build` (R-ART-16) | only with `--build` | artifacts/lock if it built |
| `velme test FILE [--goal G]` | run every `examples:` item, then the generated-input check suite (`compiler/22`), for leaf and composite goals alike, through the verifier `build` uses (R-CLI-04, D-107) | only with `--build` | artifacts/lock if it built |
| `velme explain FILE --goal G` | render the call DAG as plain-language steps (§3.4) | never | nothing |
| `velme trace FILE --goal G [input]` | `run` with the full execution trace printed (`runtime/30`) | as `run` | as `run` |
| `velme artifact FILE --goal G` | show the locked artifact: hash, manifest lines, then the IR as pretty JSON (R-CLI-22, D-107) | never | nothing |
| `velme gc` | delete artifact files under `.velme/artifacts/` (and leftover temp files) not referenced by the project's `velme.lock` (R-ART-12, R-CLI-23) | never | `.velme/artifacts/` |
| `velme cache clean` | delete the user-level WASM module cache directory if it exists (`runtime/31` R-SBX-13, D-48; R-CLI-24) | never | the user cache directory |

**R-CLI-03** There is no separate `velme lock` command: `build` (or `run`/`test`/`trace` with `--build`) is the only
writer of `velme.lock` (D-12). Without `--build`, those commands never synthesize — no surprise LLM cost (D-28).
**R-CLI-04** `--locked` forbids synthesis and any write to the lock; a missing or changed lock entry fails with `VL0702
LockStale` naming the goal, while an artifact file that is missing or damaged is `VL0701` or `VL0703` (`runtime/32`
R-ART-10, D-106). `build --locked` re-verifies every lock hit as `build` does, with no provider call; a goal that
no longer passes its examples or checks fails with `VL0702` and the cause "no longer passes its examples", and nothing is
dropped or written. Lock entries for goals no longer in the source are ignored, not an error. On `test`, `--locked`
only forbids `--build` and any write; the verification runs as always (D-107). A generated input that fails by a runtime failure is reported as that failure, with its own code and exit code, and a failed check as `VL0501`; `VL0503` is a build's alone. CI is expected to run `velme test --locked` (D-46), not only `velme run --locked`: when
`build` accepts a goal's artifact from the store unchanged (no synthesis needed) and whenever `velme test --locked`
runs, Velme re-runs that goal's `examples:` and generated-input suite before trusting it (D-46, `runtime/32` R-ART-14);
`velme run` never does — it relies on hash verification, IR validation and the manifest cross-check (`runtime/32`
R-ART-10) only.
**R-CLI-14** `--build` together with `--locked` is a usage error, `VL0902` inside the normal `--json` envelope, exit `64`,
rather than silently ignoring `--build` (the flags table already notes the combination); nothing is built or run. Any other
bad flag or flag value is `VL0902` in the same way (D-108).
**R-CLI-05** `--offline` forbids network access and constructs no provider at all, including the local `ollama`
and `external` ones (D-41). When a build needs synthesis, a stale goal fails with `VL0404 ProviderUnavailable`
instead of attempting a request. With `--offline` no contact of any kind happens: a goal that isn't fresh in the lock is
`VL0404` with the offline wording (`reference/90`), and the store is not consulted, since computing a store key can need
contact with the provider (`ollama` and `external` resolve their identity over the network, `compiler/22` R-SYNTH-25).
`replay` and `scripted` count as providers like the rest. With `--locked` and `--offline` together, `--locked` is checked
first, so a stale goal is `VL0702` (D-106).
**R-CLI-20** A source file's project root is the directory of the nearest `velme.toml` upward from the file, or the
file's own directory when there is none (D-82). `velme.lock` and `.velme/` (`runtime/32` §4) live in the project root.
The file's path is resolved first (`..`, links, and the case of each name as stored on disk), and the lock's `file`
field is that on-disk path from the root with `/` separators, so every spelling of one file names one lock entry. A name
on that path that isn't UTF-8 is `VL0901`: the lock records the path as text.
**R-CLI-06** `--goal` names must match a declared goal exactly; otherwise `VL0903 GoalNotFound` with the nearest
spelling suggestion.

### 2.1 Global flags

| Flag | Default | Meaning |
|---|---|---|
| `--json` | off | machine-readable output on stdout: envelope, diagnostics, result value, trace (schema in §3.2) |
| `--color auto\|always\|never` | `auto` | ANSI colour, decided for each stream by its own state: `auto` disables colour on a stream that is not a TTY, or when `NO_COLOR` is set (R-CLI-28) |
| `--provider NAME` | `velme.toml` → `anthropic` | synthesis provider: `anthropic`, `ollama`, `external`, `replay`, `scripted` (test builds only) |
| `--model ID` | `VELME_MODEL` / `velme.toml` | model id for `anthropic` / `ollama`; never a code constant (D-14); when `allowed_models` is set, a model outside it is `VL0405` (R-CLI-26, D-50) |
| `--external-url URL` | `VELME_EXTERNAL_URL` / user config | the base URL of the `external` backend's service (`compiler/22` §3.2, D-101); see R-CLI-13 |
| `--ollama-url URL` | `VELME_OLLAMA_URL` / user config | the base URL of the Ollama server; same source and URL rules as `--external-url` (R-CLI-13) |
| `--locked` | off | see R-CLI-04 |
| `--offline` | off | see R-CLI-05 |
| `--build` | off | let `run`/`test`/`trace` synthesize stale goals and update the lock first (D-28); together with `--locked` is a usage error (R-CLI-14); the provider flags (`--provider`, `--model`, `--external-url`, `--ollama-url`) are valid only on `build` or together with `--build` (R-CLI-21) |
| `--jobs N` | available CPUs | worker count for the DAG scheduler (`runtime/30` R-RUN-07); results are identical for every value, including 1 (INV-3) |
| `--backend interp\|wasm\|auto` | `interp` until the M7 gate, then `auto` | execution backend for leaf goal bodies in `run`/`test`/`trace` (`runtime/31` R-SBX-02, R-SBX-17); output is byte-identical (INV-3); a flag only, with no `velme.toml` key; any other value is `VL0902` (R-CLI-21, D-117) |
| `--config PATH` | nearest `velme.toml` upward from `FILE` | alternative project config: it supplies settings only and does not move the project root (R-CLI-25) |
| `-q` / `-v` | normal | `-q` drops progress lines only, never diagnostics, results or the R-SEC-12 notice; `-v` adds phase timings and cache hits |

## 3. Inputs, outputs and learner-facing output

### 3.1 Inputs (D-23)

`--input FILE.json` (or `--input -` for stdin) supplies one JSON object keyed by parameter name; `--arg name=<json>`
may repeat and overrides keys from `--input`. The JSON↔type mapping is the single mapping in `language/11`.

**R-CLI-07** Missing, extra, or mistyped arguments fail before execution with `VL0902 InvalidInput`, naming the
parameter, the expected Velme type and the JSON received. Input JSON above 16 MiB, or nested deeper than the IR
expression-depth limit (`compiler/21` R-IR-17, 128), is rejected with `VL0902` before decoding starts (T-9).

### 3.2 Output

Human mode prints the result as pretty JSON using the same mapping. `--json` prints one versioned envelope (D-49):

```json
{
  "format": "velme-cli/1",
  "status": "ok",
  "results": [
    {
      "goal": "BuildPlayerSummary", "status": "ok",
      "diagnostics": [], "result": { /* … */ }, "trace"?: { /* … */ },
      "calls"?: [ { "binding": "score", "goal": "CalculateScore", "status": "ok" } ]
    }
  ],
  "diagnostics": [],
  "notices": [],
  "summary"?: { /* build only, R-SYNTH-21 */ }
}
```

**R-CLI-08** Diagnostics carry `code`, `severity`, `message`, `file`, `span` (`start`/`end` byte offsets and 1-based
`line`/`column`, the column counted in Unicode scalar values, D-68), `labels[]` (each a `span` and `text`), `notes[]`
and `help?`. The human message and the JSON `message` are the same text.
**R-CLI-19** Every path shown to the user or written to a file — a diagnostic's `file`, a trace entry, and the lock's
`file` field (`runtime/32` §5) — is project-relative with `/` separators on every platform, including Windows; the
same project built on different OSes produces byte-identical `velme.lock`, diagnostics and traces. The one exception
is a file outside the project: a diagnostic about the user-level config carries its absolute path, and one about a
`--config` file the path as it was given, both still with `/` separators, so a tool can open the file (D-111).
**R-CLI-15** The top-level `status` is the worst of the per-goal `status` values in `results[]`, each one of
`ok | failed | pending | blocked | skipped` (`pending` = `VL0408`, `blocked` = `VL0409 SynthesisBlocked`), and is
`failed` whenever the top-level `diagnostics[]` holds an error: that array carries the diagnostics that belong to the
file rather than to one goal, such as syntax errors and an unreadable file (D-72); `notices[]`
carries the R-SEC-12 notice lines as plain strings instead of stderr. A JSON Schema,
`docs/schemas/velme-cli-1.schema.json`, is this envelope's contract; M6b writes it from the envelope as it stands, and
because it is a public contract it gets an architect review. The golden suite is every `--json` snapshot test in
`velme-cli`, each validated against the schema (AC-CLI-12), so the schema and the real output cannot drift apart (D-108).
**R-CLI-27** A `results[]` entry describes the goal that was asked for. When that goal calls others, the entry may also
hold `calls[]`: one `{binding, goal, status}` per child call, in source order (D-9), with `status` as above. Child results
and traces are not repeated there; the human output lists the same calls (§3.3). The enum values (`goalStatus`, `severity`, `kind`, the trace `version`) are closed within `velme-cli/1`: a new value needs
`velme-cli/2` (D-111). A diagnostic with no place in a source file carries in `file` the file it is about (`velme.toml`,
the user config path, `velme.lock`, or the empty string when there is none) and the span `0,0,1,1`, documented in the
schema as "no place" (D-111). Human output names the same file, or has no file header when there is none. Changes within `velme-cli/1` are additive only, as `runtime/30` R-RUN-19 already requires for the
trace schema; a breaking change ships as `velme-cli/2` (`delivery/52` R-REL-08).

### 3.3 Sample output (F-2)

```text
$ velme check player.velme
✓ Parsed
✓ Types valid
✓ Call graph valid
✓ IR valid        (3 goals locked)
✓ Checks valid

$ velme run player.velme --goal BuildPlayerSummary --input lina.json
CalculateScore      ✓
FindBadge           ✓
BuildPlayerSummary  ✓

Result:
{
  "name": "Lina",
  "score": 820,
  "badge": "Silver"
}
```

Call lines are listed in source order (D-9), never completion order.

Check failure (D-20), matching the debugging model in `runtime/30`:

```text
BuildPlayerSummary didn't pass its check  [VL0501]

  CalculateScore   ✓ 820
  FindBadge        ✓ "Silver"

  Check:    result.score == score
  Expected: 820
  Got:      8200

  help: the goal's plan produced a different score than CalculateScore gave it.
        Try `velme build player.velme` to rebuild, or make the plan more specific.
```

### 3.4 `explain`

Built from the deterministic DAG only — no LLM. Waves render as "First:", "At the same time:", "Then:",
"Finally:" with each call's goal name turned into words, followed by the child's plan's first sentence; the tail is
"Finally: " and the goal's own plan's first sentence, unquoted (`runtime/30` R-RUN-22 has the rules). Wired goals (D-4)
end with "The answer is <binding>."

### 3.5 Friendly-output style guide (P-6)

| Rule | Example |
|---|---|
| Say what happened in plain words first; code in brackets at the end | `I don't know a type called Playr  [VL0201]` |
| Point at the source with a caret span (ariadne), one primary label | — |
| Offer a concrete next step as `help:` | `help: did you mean Player?` |
| No Rust, WASM, IR, fuel or stack jargon in the first line; details go in `-v` or `--json` | "ran out of thinking steps" not "fuel exhausted" |
| Never blame the learner; never "fatal"/"illegal" | "can't" / "doesn't" |
| One diagnostic per root cause; suppress cascades from the same span | — |

**R-CLI-09** Every user-visible message string is composed by the library that detects the problem, starting from
the code's template in `reference/90` §2 (CC-CONST, D-74); the CLI never composes its own error prose.
**R-CLI-17** In human mode, every string Velme did not itself produce is escaped before display (D-47): external
the backend's response-body tail and `{"error"}` reasons (`VL0406`), `{"question"}`/`{"pending"}` text, values computed by synthesized
IR shown in `Got:` lines, trace text, and input echoes. C0/C1 control characters (other than the newline and tab a
layout expects), ESC/ANSI/OSC sequences, and Unicode bidi controls (U+061C, U+200E, U+200F, U+202A–U+202E,
U+2066–U+2069) render as visible escapes such as `\u{1b}`, so no such byte reaches the terminal. `--json` writes the
same characters as `\uXXXX` escapes (JSON itself escapes only C0), so the decoded value is unchanged (D-108). `compiler/22` R-SYNTH-33's cleaning of question/pending text is a separate, additional
length/format rule for that one field, not a substitute for this one.

## 4. Exit codes

| Code | Meaning |
|---|---|
| `0` | success (and all checks/examples passed) |
| `1` | the program is invalid: `VL01xx`–`VL03xx` |
| `2` | build could not produce an accepted artifact: `VL04xx`, `VL0503` |
| `3` | execution failed: check/example failure `VL0501/0502` or runtime failure `VL06xx` except `VL0607`, `VL0801` |
| `4` | artifact/lock problem: `VL07xx` |
| `64` | usage or input error: `VL09xx`, bad flags |
| `70` | internal error `VL0607` (a Velme bug — message asks the user to report it) |

**R-CLI-10** Exit codes are part of the CLI contract; changing a mapping is a breaking change (`delivery/52` §5).
**R-CLI-16** When goals in one invocation fail with codes from different rows above, the process exit code is decided
by precedence `70 > 64 > 1 > 4 > 2 > 3` (D-49), highest first: an internal error (`70`) always wins, then a usage
error (`64`), then an invalid program (`1`), then an artifact/lock problem (`4`), then a failed build (`2`), then an
execution failure (`3`). `pending` (`VL0408`) keeps exit code `2` — no new exit code is added for it.

## 5. Configuration

### 5.1 `velme.toml`

```toml
[project]
language = "velme/0.1"          # default when the file has no header (language/10)

[synthesis]
provider = "anthropic"          # anthropic | ollama | external | replay (`scripted` is flag-only, D-111)
model = "…"                     # required for anthropic and ollama; no built-in default constant
max_retries = 3                 # 0..=3 (compiler/22 R-SYNTH-11); external defaults to 0 (R-SYNTH-30)
timeout_secs = 60               # per provider request
max_output_tokens = 8192        # LLM providers only; the default is 8192 for anthropic and 2048 for ollama (D-110)
max_calls_per_build = 50        # hard stop across the whole build (compiler/22 R-SYNTH-21)
replay_dir = "tests/fixtures/synth"   # replay provider only; relative, inside the project root (R-CLI-18)
# token cost, LLM providers only (compiler/22 §4.1, D-44)
prompt_cache = true             # anthropic: cache the fixed prompt prefix (R-SYNTH-34)
schema_in_prompt = "summary"    # summary | full (R-SYNTH-35)
reply_format = "ir-json"        # ir-json | compact (R-SYNTH-36)
retry_history = "latest"        # latest | all (R-SYNTH-37)
stop_on_repeat = true           # stop when two attempts in a row share a cause (R-SYNTH-37)
max_prompt_examples = 8         # 0..=64 (R-SYNTH-38)
# retry_model = "…"             # optional model for retries (R-SYNTH-39)

[budget]                        # project defaults; can only tighten system caps (D-8)
cpu = "50ms"
memory = "16mb"
calls = 128
depth = 32

[artifacts]
# dir = ".velme/artifacts"      # not available yet: any value is VL0902 "isn't available yet" (a path outside the project is VL0902 too, R-CLI-18)
```

The user-level config is a separate file, read once per command (R-CLI-25). It is not a layer of defaults: it holds the
user's own choices about where plans are sent and what a build may spend, and nothing else (D-105). Its `[synthesis]`
table has exactly these keys, and any other key is `VL0902`:

```toml
[synthesis]
external_url = "https://backend.example.com/velme"  # R-CLI-13
external_ca_file = "/etc/ssl/corp-ca.pem"  # absolute path to a PEM file; added to the bundled roots for `external` only
external_timeout_secs = 30      # external provider only: each request, `describe` included (default 30)
ollama_url = "http://127.0.0.1:11434" # ollama provider only; same rules as external_url (R-CLI-13)
allowed_models = ["claude-…", "llama3.1"]  # optional; a model outside it is VL0405 (R-CLI-26)
max_calls_per_build = 20        # ceiling; a project's own value is clamped down to this, never raised (D-50)
max_retries = 1                 # ceiling
max_output_tokens = 4096        # ceiling
```

`external_ca_file` that is unreadable, or holds no PEM certificate, is `VL0901`; it does not apply to `ollama` or
`anthropic`.

**R-CLI-11** Unknown keys are an error (`VL0902`), not ignored. Precedence for a setting's value: flag > environment >
project `velme.toml` > built-in defaults; the user-level config has no place in that order, because it holds no
defaults. The service URLs (`external_url`, `ollama_url`) are the exception that proves it: they come from flag >
environment > user-level config and never from the project (R-CLI-13). System caps (`runtime/30`) and the user-level
ceilings above (D-50) are not a layer either: they clamp the resolved value afterward and can only tighten it. A project's
`model` is never replaced by the user's config; the user overrides it with `--model` or `VELME_MODEL`, and
`allowed_models` refuses one that isn't allowed (R-CLI-26, D-105).
**R-CLI-13** The `external` backend's base URL is read only from `--external-url`, `VELME_EXTERNAL_URL`, or `external_url`
in the user-level config — never from a project's `velme.toml` (`tooling/41` T-10); an `external_url` key there is
`VL0902`. The Ollama server's URL follows the same rule: `--ollama-url`, `VELME_OLLAMA_URL` or `ollama_url` in the
user-level config, never the project's, so that a cloned project cannot choose where plans are sent; an `ollama_url` key in
a project's `velme.toml` is `VL0902`, and with none given the default is `http://127.0.0.1:11434` (D-105). A URL that is not `https` or plain `http` to `localhost`,
127.0.0.0/8 or `[::1]`, that has another scheme or user information, or that does not parse is `VL0902` before any
contact; with no URL the provider is not configured (`VL0405`). The optional bearer token is read only from
`VELME_EXTERNAL_TOKEN` (§5.2). `compiler/22` R-SYNTH-29 defines how the URL and token are used.
**R-CLI-25** Configuration is read and validated by every command that takes a file (`check`, `build`, `run`, `test`,
`explain`, `trace`, `artifact`), so a bad config exits `64` whichever command found it; `gc` and `cache clean` read only
what they need (R-CLI-23). The user-level file is `$XDG_CONFIG_HOME/velme/config.toml`, else
`~/.config/velme/config.toml` on Unix and macOS, and `%APPDATA%\velme\config.toml` on Windows; a missing file is the
same as an empty one. A file that can't be read is `VL0901`. Bad TOML, a wrong type, an out-of-range value or an unknown
key, in either file, is `VL0902` worded "`{key}` in `{file}` should be {expected}, but got {found}." (`reference/90`). `--config
PATH` supplies the project settings only: it does not move the project root (R-CLI-20), and `velme.lock`, `.velme/` and
relative paths keep resolving from the source file's own root (D-105).
**R-CLI-26** When `allowed_models` is set, the resolved model (from the flag, `VELME_MODEL` or the project, and `retry_model`
too) must be in it; if not, the build fails with `VL0405` "The model `{model}` isn't in your allowed models." before any
contact, naming the list. Nothing is swapped for the first allowed model, across providers or otherwise (D-105).
**R-CLI-21** Provider flags (`--provider`, `--model`, `--external-url`, `--ollama-url`) apply only to `build` and to
`run`/`test`/`trace` with `--build`; given anywhere else they are `VL0902`, not ignored. `--backend` takes `interp`,
`wasm` or `auto`, and any other value is `VL0902`; under `--backend wasm` a leaf goal the emitter declines is `VL0607`,
never a silent fallback (`runtime/31` R-SBX-02, D-117). The output has no `backend` field, in `velme-cli/1` or in the
trace. A bad flag is reported like any input error: as a top-level diagnostic in the normal `--json` envelope, exit
`64` (D-108).
**R-CLI-22** `velme artifact FILE --goal G` loads the goal's artifact with the same checks and the same codes as `run`
(`runtime/32` R-ART-10, R-ART-16): a missing lock entry or a changed one is `VL0702`, a missing file `VL0701`, a damaged
one `VL0703`; it never shows a stale artifact. On success it prints the artifact's hash, then the manifest's fields one
`name: value` per line, then the IR as pretty JSON. With `--json` the goal result carries `{"artifact": <hash>, "manifest":
{…}, "ir": {…}}` under a sibling key `artifact`; `result` is only ever a goal's value (D-107, D-111).
**R-CLI-23** `velme gc` takes no file. Its project root is the directory of the nearest `velme.toml` upward from the
current directory, else the current directory itself. It refuses (`VL0901`) when there is no readable `velme.lock`. It
deletes only artifact files under `.velme/artifacts/` that no lock entry names, plus leftover temp files under
`.velme/tmp/`, never the lock, the log or anything else, and prints how many files it removed (D-108). It skips store and temp files modified in the last 10 minutes, so it is safe
beside a running build (D-111). `summary.removed` is a file count for `gc` and `0` or `1` (the directory removed) for
`cache clean`.
**R-CLI-24** `velme cache clean` deletes the user-level WASM module cache directory if it exists and exits `0` when it
does not, so the command ships before the cache does (M7, D-48) and needs no other change then (D-108).
**R-CLI-28** `--color auto` decides for each output stream by that stream's own state: stdout is coloured when it is a TTY,
stderr when it is a TTY, and `NO_COLOR` set to any value disables both. `always` and `never` apply to both (D-108).
**R-CLI-18** `[artifacts] dir` and `[synthesis] replay_dir` must be relative paths that stay inside the project root
(no leading `/`, no `..` component); an absolute path or one that escapes the project is `VL0902`.

### 5.2 Environment variables

| Variable | Use |
|---|---|
| `VELME_API_KEY` | provider API key (preferred); read only at request time |
| `ANTHROPIC_API_KEY` | fallback for the `anthropic` provider |
| `VELME_MODEL` | model id override |
| `VELME_EXTERNAL_URL` | the `external` backend's base URL (R-CLI-13) |
| `VELME_EXTERNAL_TOKEN` | optional bearer token for the `external` backend, sent as `Authorization: Bearer` to the configured URL only; never logged or recorded (`tooling/41` R-SEC-13) |
| `VELME_LIVE_LLM=1` | enables live-provider tests (`delivery/51`, D-13); ignored by the CLI itself |
| `VELME_SYNTH_RECORD=1` | with a live provider, records every exchange as replay fixtures in `replay_dir` (`compiler/22` R-SYNTH-43) |
| `VELME_SYNTH_SCRIPT` | path of the `scripted` provider's script file: a JSON array of entries, each a reply document or `{"error": "<variant>"}`, consumed in order across the build. Read, and `--provider scripted` accepted, only by a `velme-cli` built with the `test-provider` Cargo feature, which release builds leave off; elsewhere `scripted` is an unknown provider (`VL0902`) (D-94) |
| `VELME_OLLAMA_URL` | the Ollama server's base URL (R-CLI-13) |
| `NO_COLOR` | disables colour on both streams (R-CLI-28) |

**R-CLI-12** API keys are accepted only from the environment — never from `velme.toml`, flags or files in the project
(`tooling/41` R-SEC-05). A missing key when synthesis is needed is `VL0405 ProviderNotConfigured` with the variable
name to set.

## 6. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-CLI-01 | `velme check` on a valid file prints the five ✓ lines and exits 0; on a type error exits 1 with the `VL` code and a source caret. |
| AC-CLI-02 | `velme run --locked` with a current lock performs zero provider calls and no network I/O. |
| AC-CLI-03 | `velme run --locked` with a stale entry exits 4 with `VL0702` naming the goal, and leaves `velme.lock` unchanged. |
| AC-CLI-04 | `--arg` overrides a key from `--input`; a missing record field exits 64 with `VL0902` naming the field and expected type. |
| AC-CLI-05 | `--json` output for a failing check validates against the documented schema and contains the same message text as human mode. |
| AC-CLI-06 | Run output lists calls in source order across 100 repeated runs of a 3-way parallel goal. |
| AC-CLI-07 | `velme explain` output is byte-identical across runs and makes no provider call. |
| AC-CLI-08 | An API key present in the environment never appears in stdout, stderr, `--json`, trace or any file under `.velme/`. |
| AC-CLI-09 | An unknown key in `velme.toml` fails with `VL0902`. |
| AC-CLI-10 | `velme test` runs `examples:` items before generated inputs, for leaf and composite goals alike, and reports each failing example with given/expected/got. |
| AC-CLI-11 | `--build --locked` exits 64 as a usage error and neither builds nor runs anything. |
| AC-CLI-12 | Every `--json` snapshot test in `velme-cli` (the golden suite, R-CLI-15) validates against `docs/schemas/velme-cli-1.schema.json`, which M6b writes. |
| AC-CLI-13 | `velme run --locked` with a stale entry for the goal and a bad `--input` exits with the usage-error code (`64`), not the lock-staleness code (`4`) — R-CLI-16 precedence. |
| AC-CLI-14 | An external backend whose response body contains `\x1b]52;c;…\x07` and a U+202E override renders both visibly (e.g. `\u{1b}`) in human mode and the terminal receives no raw ESC byte; `--json` carries the same characters as `\u001b` and `\u202e` escapes, and the decoded value is unchanged. |
| AC-CLI-15 | `[artifacts] dir = "/etc"` and `replay_dir = "../outside"` each fail with `VL0902`. |
| AC-CLI-16 | Building the same project on a Windows-style and a Linux-style path layout produces byte-identical `velme.lock` `file` fields, using `/` on both. |
| AC-CLI-17 | A user-level `max_calls_per_build` ceiling below a project's `[synthesis] max_calls_per_build` makes the build stop at the ceiling, and the R-SEC-12 notice names both values. |
| AC-CLI-18 | `velme build --locked` on a project whose locked artifact was edited to break one of its examples (hash-consistent) fails with `VL0702` and the cause "no longer passes its examples", makes no provider call, and leaves `velme.lock` and the artifacts unchanged; a lock entry for a deleted goal is ignored and is no error. |
| AC-CLI-19 | `velme build --offline` with a goal that isn't fresh in the lock fails with `VL0404` in the offline wording, for `anthropic`, `ollama`, `external`, `replay` and `scripted` alike, with no contact and no store lookup; adding `--locked` makes it `VL0702`. |
| AC-CLI-20 | `velme artifact` prints the hash, the manifest lines and the pretty IR, and with `--json` the `{artifact, manifest, ir}` result; a stale, missing or damaged artifact fails with `VL0702`, `VL0701` or `VL0703` as `run` does. |
| AC-CLI-21 | `velme gc` run from a subdirectory of a project deletes exactly the unreferenced artifact files and temp files and prints the count; with no `velme.lock` it refuses. `velme cache clean` exits 0 with no cache directory present. |
| AC-CLI-22 | An `ollama_url` or `external_url` in a project's `velme.toml`, an unknown key in the user-level file, and a wrong type or out-of-range value in either each fail with `VL0902` and the "should be … but got …" wording; an unreadable config or `external_ca_file` is `VL0901`; a user-level file is found at the R-CLI-25 location for each platform. |
| AC-CLI-23 | A model outside `allowed_models`, from the flag, `VELME_MODEL` or the project, fails with `VL0405` and is never swapped. |
| AC-CLI-24 | `--provider` on `velme check`, an unknown `--backend` value and an invalid flag value each exit 64 with `VL0902`, inside the `--json` envelope when `--json` is given; `-q` removes progress lines but not diagnostics, results or the R-SEC-12 notice. |
| AC-CLI-25 | `--color auto` colours stderr but not stdout when only stderr is a TTY, and neither with `NO_COLOR` set. |
| AC-CLI-26 | `run --json` for a composite goal has one `results[]` entry for the requested goal, with `calls[]` in source order holding `binding`, `goal` and `status`. |
