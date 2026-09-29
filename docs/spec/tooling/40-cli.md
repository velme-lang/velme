# 40 — Command-Line Interface

**Status:** v0.1 · **Area:** CLI
**Read when:** adding or changing a `velme` command, flag, config key, environment variable, exit code or output format.
**Depends on:** [SPEC](../SPEC.md), [11-types](../language/11-types.md) (JSON mapping), [30-execution-vibevm](../runtime/30-execution-vibevm.md), [32-artifacts-cache](../runtime/32-artifacts-cache.md), [90-errors-glossary](../reference/90-errors-glossary.md)
**Source:** §34, §42A.3, §47, §48

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
| `velme test FILE [--goal G]` | run every `examples:` item, then the generated-input check suite (`compiler/22`) | only with `--build` | artifacts/lock if it built |
| `velme explain FILE --goal G` | render the call DAG as plain-language steps (§3.4) | never | nothing |
| `velme trace FILE --goal G [input]` | `run` with the full execution trace printed (`runtime/30`) | as `run` | as `run` |
| `velme artifact FILE --goal G` | show the locked artifact: hash, manifest versions, IR (pretty JSON) | never | nothing |
| `velme gc` | delete artifacts under `.velme/artifacts/` not referenced by the project's `velme.lock` (R-ART-12) | never | `.velme/artifacts/` |
| `velme cache clean` | delete the user-level WASM module cache (`runtime/31` R-SBX-13, D-48) | never | the user cache directory |

**R-CLI-03** There is no separate `velme lock` command: `build` (or `run`/`test`/`trace` with `--build`) is the only
writer of `velme.lock` (D-12). Without `--build`, those commands never synthesize — no surprise LLM cost (D-28).
**R-CLI-04** `--locked` forbids synthesis and any write to the lock; a missing or stale entry fails with `VL0702
LockStale` naming the goal. CI is expected to run `velme test --locked` (D-46), not only `velme run --locked`: when
`build` accepts a goal's artifact from the store unchanged (no synthesis needed) and whenever `velme test --locked`
runs, Velme re-runs that goal's `examples:` and generated-input suite before trusting it (D-46, `runtime/32` R-ART-14);
`velme run` never does — it relies on hash verification, IR validation and the manifest cross-check (`runtime/32`
R-ART-10) only.
**R-CLI-14** `--build` together with `--locked` is a usage error, exit `64`, rather than silently ignoring `--build`
(the flags table already notes the combination).
**R-CLI-05** `--offline` forbids network access and constructs no provider at all, including the local `ollama`
and `external` ones (D-41). When a build needs synthesis, a stale goal fails with `VL0404 ProviderUnavailable`
instead of attempting a request.
**R-CLI-20** A source file's project root is the directory of the nearest `velme.toml` upward from the file, or the
file's own directory when there is none (D-82). `velme.lock` and `.velme/` (`runtime/32` §4) live in the project root.
The file's path is resolved first (`..`, links, and the case of each name as stored on disk), and the lock's `file`
field is that on-disk path from the root with `/` separators, so every spelling of one file names one lock entry.
**R-CLI-06** `--goal` names must match a declared goal exactly; otherwise `VL0903 GoalNotFound` with the nearest
spelling suggestion.

### 2.1 Global flags

| Flag | Default | Meaning |
|---|---|---|
| `--json` | off | machine-readable output on stdout: envelope, diagnostics, result value, trace (schema in §3.2) |
| `--color auto\|always\|never` | `auto` | ANSI colour; `auto` disables when stdout is not a TTY or `NO_COLOR` is set |
| `--provider NAME` | `velme.toml` → `anthropic` | synthesis provider: `anthropic`, `ollama`, `external`, `replay`, `scripted` (test builds only) |
| `--model ID` | `VELME_MODEL` / `velme.toml` | model id for `anthropic` / `ollama`; never a code constant (D-14); clamped to any `allowed_models` ceiling (D-50) |
| `--external-command CMD` | `VELME_EXTERNAL_COMMAND` / user config | the `external` backend's command line (`compiler/22` §3.2); see R-CLI-13 |
| `--locked` | off | see R-CLI-04 |
| `--offline` | off | see R-CLI-05 |
| `--build` | off | let `run`/`test`/`trace` synthesize stale goals and update the lock first (D-28); together with `--locked` is a usage error (R-CLI-14) |
| `--jobs N` | available CPUs | worker count for the DAG scheduler (`runtime/30` §6); results are identical for every value, including 1 (INV-3) |
| `--backend interp\|wasm` | `interp` until M7, then `wasm` for leaf goals | execution backend for `run`/`test`/`trace` (`runtime/31`); results must be identical (INV-3) |
| `--config PATH` | nearest `velme.toml` upward from `FILE` | alternative project config |
| `-q` / `-v` | normal | quieter / add phase timings and cache hits |

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
      "diagnostics": [], "result": { /* … */ }, "trace"?: { /* … */ }
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
same project built on different OSes produces byte-identical `velme.lock`, diagnostics and traces.
**R-CLI-15** The top-level `status` is the worst of the per-goal `status` values in `results[]`, each one of
`ok | failed | pending | blocked | skipped` (`pending` = `VL0408`, `blocked` = `VL0409 SynthesisBlocked`), and is
`failed` whenever the top-level `diagnostics[]` holds an error: that array carries the diagnostics that belong to the
file rather than to one goal, such as syntax errors and an unreadable file (D-72); `notices[]`
carries the R-SEC-12 notice lines as plain strings instead of stderr. A committed JSON Schema
(`docs/schemas/velme-cli-1.schema.json`) is this envelope's contract, checked by a golden test against real
`--json` output. Changes within `velme-cli/1` are additive only, as `runtime/30` R-RUN-19 already requires for the
trace schema; a breaking change ships as `velme-cli/2` (`delivery/52` R-REL-08).

### 3.3 Sample output (corrected §47, F-2)

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

Built from the deterministic DAG only — no LLM (§34). Waves render as "First:", "At the same time:", "Then:",
"Finally:" with each call's goal name turned into words, and the plan text quoted for the tail. Wired goals (D-4) end
with "The answer is <binding>."

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
backend stderr and `{"error"}` reasons (`VL0406`), `{"question"}`/`{"pending"}` text, values computed by synthesized
IR shown in `Got:` lines, trace text, and input echoes. C0/C1 control characters (other than the newline and tab a
layout expects), ESC/ANSI/OSC sequences, and Unicode bidi controls (U+061C, U+200E, U+200F, U+202A–U+202E,
U+2066–U+2069) render as visible escapes such as `\u{1b}`, so no such byte reaches the terminal. `--json` writes the
same characters as `\uXXXX` escapes (JSON itself escapes only C0), so the decoded value is unchanged. `compiler/22` R-SYNTH-33's cleaning of question/pending text is a separate, additional
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
provider = "anthropic"          # anthropic | ollama | external | replay
model = "…"                     # required for anthropic and ollama; no built-in default constant
max_retries = 3                 # 0..=3 (compiler/22 R-SYNTH-11); external defaults to 0 (R-SYNTH-30)
timeout_secs = 60               # per provider request
max_output_tokens = 8192        # LLM providers only
max_calls_per_build = 50        # hard stop across the whole build (compiler/22 R-SYNTH-21)
replay_dir = "tests/fixtures/synth"   # replay provider only; relative, inside the project root (R-CLI-18)
ollama_url = "http://127.0.0.1:11434" # ollama provider only
external_timeout_secs = 30      # external provider only; the command itself is never set here (R-CLI-13)
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
dir = ".velme/artifacts"        # relative, inside the project root (R-CLI-18)
```

User-level config (`$XDG_CONFIG_HOME/velme/config.toml`, or the platform equivalent) may additionally set spending
ceilings that every project's `[synthesis]` values are clamped to (tighten only, D-50):

```toml
[synthesis]
external_command = ["/usr/local/bin/impl", "--queue", "velme"]  # R-CLI-13
allowed_models = ["claude-…", "llama3.1"]  # optional; a project's `model` outside it is clamped to the first entry
max_calls_per_build = 20        # ceiling; a project's own value is clamped down to this, never raised
max_retries = 1                 # ceiling
max_output_tokens = 4096        # ceiling
```

**R-CLI-11** Unknown keys are an error (`VL0902`), not ignored. Precedence for a setting's value: flag > environment >
project `velme.toml` > user-level config > built-in defaults. System caps (`runtime/30`) and the user-level ceilings
above (D-50) are not a layer in that order: they clamp the resolved value afterward and can only tighten it.
**R-CLI-13** The `external` command is read only from `--external-command`, `VELME_EXTERNAL_COMMAND`, or
`external_command` in the user-level config — never from a project's `velme.toml` (`tooling/41` T-10); an
`external_command` key there is an unknown key (`VL0902`). It is an argument list (as in the example above);
`--external-command` and `VELME_EXTERNAL_COMMAND` are split into arguments shell-words style (quoting only — no
globbing, and no variable or `~` expansion). `compiler/22` R-SYNTH-29 defines how the resulting program is resolved
and run.
**R-CLI-18** `[artifacts] dir` and `[synthesis] replay_dir` must be relative paths that stay inside the project root
(no leading `/`, no `..` component); an absolute path or one that escapes the project is `VL0902`.

### 5.2 Environment variables

| Variable | Use |
|---|---|
| `VELME_API_KEY` | provider API key (preferred); read only at request time |
| `ANTHROPIC_API_KEY` | fallback for the `anthropic` provider |
| `VELME_MODEL` | model id override |
| `VELME_EXTERNAL_COMMAND` | the `external` backend's command line (R-CLI-13) |
| `VELME_LIVE_LLM=1` | enables live-provider tests (`delivery/51`, D-13); ignored by the CLI itself |
| `NO_COLOR` | disables colour |

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
| AC-CLI-10 | `velme test` runs `examples:` items before generated inputs and reports each failing example with given/expected/got. |
| AC-CLI-11 | `--build --locked` exits 64 as a usage error and neither builds nor runs anything. |
| AC-CLI-12 | Every `--json` fixture in the golden suite validates against `docs/schemas/velme-cli-1.schema.json`. |
| AC-CLI-13 | `velme run --locked` with a stale entry for the goal and a bad `--input` exits with the usage-error code (`64`), not the lock-staleness code (`4`) — R-CLI-16 precedence. |
| AC-CLI-14 | An external backend whose stderr contains `\x1b]52;c;…\x07` and a U+202E override renders both visibly (e.g. `\u{1b}`) in human mode and the terminal receives no raw ESC byte; `--json` carries the raw text unescaped beyond normal JSON escaping. |
| AC-CLI-15 | `[artifacts] dir = "/etc"` and `replay_dir = "../outside"` each fail with `VL0902`. |
| AC-CLI-16 | Building the same project on a Windows-style and a Linux-style path layout produces byte-identical `velme.lock` `file` fields, using `/` on both. |
| AC-CLI-17 | A user-level `max_calls_per_build` ceiling below a project's `[synthesis] max_calls_per_build` makes the build stop at the ceiling, and the R-SEC-12 notice names both values. |
