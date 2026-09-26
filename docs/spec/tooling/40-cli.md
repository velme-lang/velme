# 40 — Command-Line Interface

**Status:** v0.1 · **Area:** CLI
**Read when:** adding or changing a `thela` command, flag, config key, environment variable, exit code or output format.
**Depends on:** [SPEC](../SPEC.md), [11-types](../language/11-types.md) (JSON mapping), [30-execution-vibevm](../runtime/30-execution-vibevm.md), [32-artifacts-cache](../runtime/32-artifacts-cache.md), [90-errors-glossary](../reference/90-errors-glossary.md)
**Source:** §34, §42A.3, §47, §48

## 1. Purpose & boundaries

Defines the `thela` binary: its commands, flags, configuration, input/output formats and how results and failures are
shown. The CLI is a thin shell over library APIs (INV-9): it parses arguments, loads config, calls `thela-sema`,
`thela-synth` and `thela-runtime`, and renders. It owns no language semantics.

**R-CLI-01** `thela-cli` contains no parsing, typing, synthesis or execution logic; every command is a call into a core
crate that the IDE, playground or a build server could make identically.
**R-CLI-02** Every command works offline when it needs no synthesis (`check`, `explain`, `artifact`, and `run`/`test`
against a current lock) — INV-7.

## 2. Commands

| Command | Does | Synthesizes? | Writes |
|---|---|---|---|
| `thela check FILE` | parse, name/type check, call-graph check, validate locked IR if present | never | nothing |
| `thela build FILE` | `check`, then synthesize + verify every goal whose lock entry is missing or stale | yes, stale goals only | `.thela/artifacts/`, `thela.lock` |
| `thela run FILE --goal G [input]` | execute `G` with the locked artifacts; a stale/missing goal is `TL0702`/`TL0701` with a hint to run `thela build` (R-ART-16) | only with `--build` | artifacts/lock if it built |
| `thela test FILE [--goal G]` | run every `examples:` item, then the generated-input check suite (`compiler/22`) | only with `--build` | artifacts/lock if it built |
| `thela explain FILE --goal G` | render the call DAG as plain-language steps (§3.4) | never | nothing |
| `thela trace FILE --goal G [input]` | `run` with the full execution trace printed (`runtime/30`) | as `run` | as `run` |
| `thela artifact FILE --goal G` | show the locked artifact: hash, manifest versions, IR (pretty JSON) | never | nothing |

**R-CLI-03** There is no separate `thela lock` command: `build` (or `run`/`test`/`trace` with `--build`) is the only
writer of `thela.lock` (D-12). Without `--build`, those commands never synthesize — no surprise LLM cost (D-28).
**R-CLI-04** `--locked` forbids synthesis and any write to the lock; a missing or stale entry fails with `TL0702
LockStale` naming the goal. CI always passes `--locked`.
**R-CLI-05** `--offline` forbids network access and constructs no provider at all, including the local `ollama`
and `external` ones (D-41). When a build needs synthesis, a stale goal fails with `TL0404 ProviderUnavailable`
instead of attempting a request.
**R-CLI-06** `--goal` names must match a declared goal exactly; otherwise `TL0903 GoalNotFound` with the nearest
spelling suggestion.

### 2.1 Global flags

| Flag | Default | Meaning |
|---|---|---|
| `--json` | off | machine-readable output on stdout: diagnostics array, result value, trace (schemas in §4) |
| `--color auto\|always\|never` | `auto` | ANSI colour; `auto` disables when stdout is not a TTY or `NO_COLOR` is set |
| `--provider NAME` | `thela.toml` → `anthropic` | synthesis provider: `anthropic`, `ollama`, `external`, `replay`, `scripted` (test builds only) |
| `--model ID` | `thela.toml` / `THELA_MODEL` | model id for `anthropic` / `ollama`; never a code constant (D-14) |
| `--external-command CMD` | `THELA_EXTERNAL_COMMAND` / user config | the `external` backend's command line (`compiler/22` §3.2); see R-CLI-13 |
| `--locked` | off | see R-CLI-04 |
| `--offline` | off | see R-CLI-05 |
| `--build` | off | let `run`/`test`/`trace` synthesize stale goals and update the lock first (D-28); ignored with `--locked` |
| `--backend interp\|wasm` | `interp` until M7, then `wasm` for leaf goals | execution backend for `run`/`test`/`trace` (`runtime/31`); results must be identical (INV-3) |
| `--config PATH` | nearest `thela.toml` upward from `FILE` | alternative project config |
| `-q` / `-v` | normal | quieter / add phase timings and cache hits |

## 3. Inputs, outputs and learner-facing output

### 3.1 Inputs (D-23)

`--input FILE.json` (or `--input -` for stdin) supplies one JSON object keyed by parameter name; `--arg name=<json>`
may repeat and overrides keys from `--input`. The JSON↔type mapping is the single mapping in `language/11`.

**R-CLI-07** Missing, extra, or mistyped arguments fail before execution with `TL0902 InvalidInput`, naming the
parameter, the expected Thela type and the JSON received.

### 3.2 Output

Human mode prints the result as pretty JSON using the same mapping. `--json` prints one object:
`{ "status": "ok"|"error", "result"?: <value>, "diagnostics": [...], "trace"?: {...} }`.

**R-CLI-08** Diagnostics in `--json` carry `code`, `severity`, `message`, `file`, `span` (byte offsets and 1-based
line/column), `notes[]` and `help?`. The human message and the JSON `message` are the same text.

### 3.3 Sample output (corrected §47, F-2)

```text
$ thela check player.thela
✓ Parsed
✓ Types valid
✓ Call graph valid
✓ IR valid        (3 goals locked)
✓ Checks valid

$ thela run player.thela --goal BuildPlayerSummary --input lina.json
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
BuildPlayerSummary didn't pass its check  [TL0501]

  CalculateScore   ✓ 820
  FindBadge        ✓ "Silver"

  Check:    result.score == score
  Expected: 820
  Got:      8200

  help: the goal's plan produced a different score than CalculateScore gave it.
        Try `thela build player.thela` to rebuild, or make the plan more specific.
```

### 3.4 `explain`

Built from the deterministic DAG only — no LLM (§34). Waves render as "First:", "At the same time:", "Then:",
"Finally:" with each call's goal name turned into words, and the plan text quoted for the tail. Wired goals (D-4) end
with "The answer is <binding>."

### 3.5 Friendly-output style guide (P-6)

| Rule | Example |
|---|---|
| Say what happened in plain words first; code in brackets at the end | `I don't know a type called Playr  [TL0201]` |
| Point at the source with a caret span (ariadne), one primary label | — |
| Offer a concrete next step as `help:` | `help: did you mean Player?` |
| No Rust, WASM, IR, fuel or stack jargon in the first line; details go in `-v` or `--json` | "ran out of thinking steps" not "fuel exhausted" |
| Never blame the learner; never "fatal"/"illegal" | "can't" / "doesn't" |
| One diagnostic per root cause; suppress cascades from the same span | — |

**R-CLI-09** Every user-visible message string comes from the diagnostic's definition in `thela-diagnostics`
(CC-CONST); the CLI never composes its own error prose.

## 4. Exit codes

| Code | Meaning |
|---|---|
| `0` | success (and all checks/examples passed) |
| `1` | the program is invalid: `TL01xx`–`TL03xx` |
| `2` | build could not produce an accepted artifact: `TL04xx`, `TL0503` |
| `3` | execution failed: check/example failure `TL0501/0502` or runtime failure `TL06xx`, `TL0801` |
| `4` | artifact/lock problem: `TL07xx` |
| `64` | usage or input error: `TL09xx`, bad flags |
| `70` | internal error `TL0607` (a Thela bug — message asks the user to report it) |

**R-CLI-10** Exit codes are part of the CLI contract; changing a mapping is a breaking change (`delivery/52` §5).

## 5. Configuration

### 5.1 `thela.toml`

```toml
[project]
language = "thela/0.1"          # default when the file has no header (language/10)

[synthesis]
provider = "anthropic"          # anthropic | ollama | external | replay
model = "…"                     # required for anthropic and ollama; no built-in default constant
max_retries = 3                 # 0..=3 (compiler/22 R-SYNTH-11); external defaults to 0 (R-SYNTH-30)
timeout_secs = 60               # per provider request
max_output_tokens = 8192        # LLM providers only
max_calls_per_build = 50        # hard stop across the whole build (compiler/22 R-SYNTH-21)
replay_dir = "tests/fixtures/synth"   # replay provider only
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
dir = ".thela/artifacts"
```

**R-CLI-11** Unknown keys are an error (`TL0902`), not ignored. Precedence: flag > environment > `thela.toml` > built-in
system caps (`runtime/30`).
**R-CLI-13** The `external` command is read only from `--external-command`, `THELA_EXTERNAL_COMMAND`, or
`external_command` in the user-level config (`$XDG_CONFIG_HOME/thela/config.toml`, or the platform equivalent).
A project file can't name a program for `thela build` to run, so building a cloned project never executes code from
it (`tooling/41` T-10). `external_command` in a project `thela.toml` is an unknown key (`TL0902`).

### 5.2 Environment variables

| Variable | Use |
|---|---|
| `THELA_API_KEY` | provider API key (preferred); read only at request time |
| `ANTHROPIC_API_KEY` | fallback for the `anthropic` provider |
| `THELA_MODEL` | model id override |
| `THELA_EXTERNAL_COMMAND` | the `external` backend's command line (R-CLI-13) |
| `THELA_LIVE_LLM=1` | enables live-provider tests (`delivery/51`, D-13); ignored by the CLI itself |
| `NO_COLOR` | disables colour |

**R-CLI-12** API keys are accepted only from the environment — never from `thela.toml`, flags or files in the project
(`tooling/41` R-SEC-05). A missing key when synthesis is needed is `TL0405 ProviderNotConfigured` with the variable
name to set.

## 6. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-CLI-01 | `thela check` on a valid file prints the five ✓ lines and exits 0; on a type error exits 1 with the `TL` code and a source caret. |
| AC-CLI-02 | `thela run --locked` with a current lock performs zero provider calls and no network I/O. |
| AC-CLI-03 | `thela run --locked` with a stale entry exits 4 with `TL0702` naming the goal, and leaves `thela.lock` unchanged. |
| AC-CLI-04 | `--arg` overrides a key from `--input`; a missing record field exits 64 with `TL0902` naming the field and expected type. |
| AC-CLI-05 | `--json` output for a failing check validates against the documented schema and contains the same message text as human mode. |
| AC-CLI-06 | Run output lists calls in source order across 100 repeated runs of a 3-way parallel goal. |
| AC-CLI-07 | `thela explain` output is byte-identical across runs and makes no provider call. |
| AC-CLI-08 | An API key present in the environment never appears in stdout, stderr, `--json`, trace or any file under `.thela/`. |
| AC-CLI-09 | An unknown key in `thela.toml` fails with `TL0902`. |
| AC-CLI-10 | `thela test` runs `examples:` items before generated inputs and reports each failing example with given/expected/got. |
