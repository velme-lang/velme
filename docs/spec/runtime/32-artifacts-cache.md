# 32 — Artifacts, Fingerprints, Lockfile & Cache

**Status:** v0.1 · **Area:** ART
**Read when:** computing a fingerprint, reading/writing an artifact or `velme.lock`, deciding whether synthesis is needed, or touching the WASM/derived caches or Coach.
**Depends on:** [SPEC](../SPEC.md), [compiler/21](../compiler/21-ir.md), [compiler/22](../compiler/22-spellbook-synthesis.md), [30-execution-vibevm](30-execution-vibevm.md)

## 1. Purpose & boundaries

An **artifact** is a verified IR goal plus the manifest that says how it was produced. Artifacts are content-addressed
and immutable (INV-8). `velme.lock` pins which artifact each goal uses, which is what makes runs reproducible even
though LLM output is not (D-12). This file owns identities, formats and staleness; producing artifacts is
[compiler/22](../compiler/22-spellbook-synthesis.md), running them is [30](30-execution-vibevm.md).

## 2. Identities (D-11, D-21)

All hashes are BLAKE3 over canonical JSON (21 R-IR-21), written `b3:<hex>`.

| Identity | Hash of | Used for |
|---|---|---|
| `signature` | goal name, param names + types, output type, every reachable record type | `Call.goal_signature`; parents' keys |
| `contract_key` | normalized goal source (signature, plan per D-21, `call` bindings, checks, examples, budget) + child `signature`s + `language_version` + the compatibility units of `ir_version` and `builtins_version` (D-55, D-85) | lock staleness (§5); generated-input seed (22 §7) |
| `synthesis_key` | `contract_key` + provider `input_version` (`prompt_version` + options hash, `compiler/22` R-SYNTH-40, or the external `request_version`) + compiler `MAJOR.MINOR` + provider id + model id (Ollama `<model>@<digest>`, external `<backend>@<backend_version>`; `compiler/22` §3) | artifact-store lookup; replay fixture name |
| `artifact` | the canonical artifact document (§3) | store address, lock pin |
| `execution_id` | `artifact` + children's `execution_id`s in binding order | exact tree identity; shown as `artifact_id` in traces |

**R-ART-01** Nothing is cached, looked up or pinned by goal name alone (INV-8).
**R-ART-02** Child **signatures**, not child artifacts, enter a parent's keys (D-11): regenerating `CalculateScore`
leaves `BuildPlayerSummary`'s keys and artifact unchanged; only its `execution_id` changes.
**R-ART-22** When `velme build` changes a child's artifact, it re-runs every ancestor composite's `examples` and
`check`s against the new children, with no LLM call (D-55). An ancestor that now fails is stale and goes through
ordinary synthesis (`compiler/22`); its diagnostic names the child that changed. If that synthesis fails too, the
build drops that ancestor's lock entry (only where its locked artifact failed an example or check in this build, not on
a watchdog stop or internal error, which fail the goal and keep the entry, INV-3). Goals above it are blocked and keep
theirs; `velme run` fails on them naming the goal with no entry (`VL0702`, R-ART-16, D-100). A goal whose own contract
changed and whose synthesis failed keeps its stale entry. This way a lock never pins a tree that was not verified
together.
A leaf goal's lock hit is checked the same way: its examples, checks and generated inputs run again against the locked
IR with no provider call, and a leaf that fails is stale and goes through ordinary synthesis (D-100). An unchanged,
passing project still makes no provider call (AC-RDM-08). Because every build re-verifies every locked leaf, a fully
locked build on a slow machine can stop with `VL0603` (the watchdog); the goal fails and its lock entry is kept. Under `--offline` a lock hit that fails re-verification also keeps its entry (D-106).
**R-ART-03** A verified artifact stays valid when only the prompt, compiler patch/minor, provider or model changes:
those enter `synthesis_key` (cache reuse) but not `contract_key` (validity). Switching models never forces
re-synthesis of a locked project.
**R-ART-04** Compiler patch versions never enter any key; `compiler_version` is recorded in the manifest only.
**R-ART-23** `contract_key` hashes `{signature, plan, calls, checks, examples, budget, language_version,
ir_compatibility, builtins_compatibility}` (D-81), the last two each version's compatibility unit — MAJOR, or
MAJOR.MINOR while MAJOR is 0 (D-85). Source enters as span-free structure, so layout and comments never change it; a
quantifier variable is its nesting depth, a number literal its R-TYP-08 value, an operator or quantifier its surface
spelling, a record by its type and field names; `calls` hold each binding, callee name, callee `signature` and
arguments; `budget` is the effective limits, system caps included: `max_fuel`, `max_memory`, `max_goal_calls`,
`max_call_depth`, `max_list_size` and `max_output_bytes`. The document is built field by field, never from a Rust
type's derived serialization, and a hard-coded golden key pins it: changing it invalidates every lock.

## 3. Artifact document & manifest

```json
{
  "manifest": {
    "format": "velme-artifact/1",
    "goal": "FindBadge", "kind": "leaf",
    "signature": "b3:…", "contract_key": "b3:…", "synthesis_key": "b3:…",
    "language_version": "0.1", "compiler_version": "0.1.4", "ir_version": "0.1",
    "builtins_version": "0.1", "prompt_version": "leaf-v1+b3:…",
    "provider": "anthropic", "model_version": "<provider-reported id>",
    "children": [],
    "verification": { "examples": 3, "generated_inputs": 61, "input_set": "b3:…", "max_fuel_observed": 412 }
  },
  "ir": { /* IR goal, 21 §2 */ }
}
```

**R-ART-05** The artifact document is deterministic: no timestamps, hostnames, usernames, token counts or retry
counts (those go to the local synth log, 22 R-SYNTH-23). Two machines accepting the same IR with the same compiler
version produce the same `artifact` hash; the manifest records `compiler_version`, so another compiler version gives
another hash (D-86).
**R-ART-24** `format` is `"velme-artifact/1"`, the artifact format; a manifest without it or with another value is no
artifact this build reads, and is rejected like any document that is no artifact (R-ART-10, D-86). `kind` is `leaf`,
`composite` or `wired`, the artifact's own spelling.
**R-ART-06** `stdlib_version` is `builtins_version` in v0.1 — there is no separate standard library.
**R-ART-07** Wired goals (D-4) produce artifacts too, with `"provider": "compiler"` and no `model_version`, so every
goal resolves through the lock the same way.
**R-ART-21** An `external` artifact records `"provider": "external"`, the backend name as `backend`, `model_version` =
`<backend>@<backend_version>` and `prompt_version` = `request_version`; an `ollama` artifact records `model_version` =
`<model>@<digest>` (D-41, D-42).
**R-ART-25** The manifest's `prompt_version` records the provider's whole `input_version` — for an LLM provider the
`prompt_version` plus the options hash (`compiler/22` R-SYNTH-40), not the prompt version alone — so `synthesis_key`
can be recomputed from the manifest (D-97).
**R-ART-08** `children` lists `{binding, goal, signature}` in source order, mirroring the IR `calls`.

## 4. Local store layout

```
<project>/
  velme.toml                   config (22 §8)
  velme.lock                   pins — commit
  .velme/
    artifacts/b3-<hex>.json   verified artifacts — commit (D-12)
    cache/wasm/…              derived modules (31 §7) — ignore
    synth-log.jsonl           local attempt log — ignore
    tmp/                      in-progress artifact writes (R-ART-09) — ignore
```

`<project>` is the project root: the directory of the nearest `velme.toml` upward from the source file, else the file's
own directory (`tooling/40` R-CLI-20, D-82).

**R-ART-09** The store is append-only and write-once: a file is written to a temp name under `.velme/tmp/` and
atomically placed; an existing file with the same name and the same bytes is never rewritten. Write-once protects
content, not names: a regular file whose bytes don't match its name isn't that artifact, so storing the artifact
replaces it whole, atomically; anything else under the name — a link, a directory, an unreadable file — is left alone
and is `VL0901`. An artifact whose canonical JSON R-ART-10 would refuse to read back is not stored. Velme reads and
writes only regular files, never through a symbolic link: a stored file or `velme.lock` is opened without following a
link and without blocking (on Unix), then checked to be a regular file, else `VL0901`; a `.velme`, `.velme/artifacts`
or `.velme/tmp` that is a link is `VL0901` too, checked before use — best effort, since a directory swapped for a link
after that check is not caught.
**R-ART-10** Every load re-hashes the file, re-runs IR validation (21 §6, cached per hash for the process), and
cross-checks the manifest's `goal`, `signature` and `contract_key` against the lock entry and against the key computed
from current source (D-46): the manifest's claims are informational, never trusted on their own. A read stops past the
largest artifact, 1 MiB of IR (21 §7) plus 64 KiB for the manifest, and past 16 MiB for `velme.lock`; the document
nests at most 513 JSON levels, the 512 IR may nest (`MAX_JSON_DEPTH`) under the `ir` member. A hash mismatch,
a file past that size, or bytes that hash to their name but aren't the canonical JSON the store writes are
`VL0703 ArtifactCorrupt`; a missing file is `VL0701 ArtifactUnavailable`; a document of another `format` (R-ART-24), a
manifest/lock/computed mismatch or a re-validation failure makes the entry stale (R-ART-14), which `run`, `test`,
`artifact` and `--locked` report as `VL0702` (D-106). So a missing or damaged file is `VL0701`/`VL0703`, and a missing or
changed lock entry is `VL0702`; "stale" only ever means that `build` re-synthesizes.
**R-ART-11** Only candidates that passed the full verification pipeline (22 §6) are written. A failed or
timed-out candidate leaves no file.
**R-ART-12** Artifacts not referenced by `velme.lock` may be garbage-collected by a CLI command (tooling/40); nothing is
deleted implicitly.

## 5. `velme.lock` (D-12)

```toml
# Generated by `velme build`. Commit this file. Do not edit by hand.
version  = 1
language = "0.1"

[[goal]]
file          = "player.velme"
name          = "BuildPlayerSummary"
signature     = "b3:3a9e…"
contract_key  = "b3:77c0…"
artifact      = "b3:c41d…"

[[goal]]
file          = "player.velme"
name          = "CalculateScore"
signature     = "b3:5f1c…"
contract_key  = "b3:09ab…"
artifact      = "b3:e2f7…"
```

**R-ART-13** Entries are sorted by (`file`, `name`); the file is rewritten byte-deterministically, so an unchanged
build produces no diff.
**R-ART-14** An entry is **stale** when its `contract_key` differs from the one computed from current source, when its
artifact is missing/corrupt, or when the R-ART-10 cross-check finds the manifest's `goal`/`signature`/`contract_key`
disagreeing with the lock entry or fails re-validation (D-46). A missing or corrupt artifact file makes it stale for `build` only, which then re-synthesizes; everywhere else that file is `VL0701`/`VL0703` (R-ART-10). The CLI names the cause by comparing key components
("the plan changed", "`Player` gained a field", "built for language 0.1, file says 0.2", "the stored artifact doesn't
match its own lock entry").
**R-ART-15** `velme build` re-synthesizes only stale or missing goals (post-order, 22 R-SYNTH-01), updates their entries
and prunes entries for deleted goals. `--locked` performs no synthesis and fails with `VL0702 LockStale` listing every
goal whose entry is missing or changed, or whose locked artifact no longer passes its examples (re-verified as `build`
does, with nothing dropped or written), and its cause; a missing or damaged artifact file is `VL0701`/`VL0703` instead,
and entries for deleted goals are ignored (D-106). CI runs with `--locked`.
**R-ART-16** `velme run`/`velme test` never synthesize implicitly unless the user passes `--build`; by default a stale or
missing lock entry is `VL0702`, and a missing or damaged artifact file `VL0701`/`VL0703`, each with a hint to run `velme build`
(D-106). (Keeps "run" free of surprise LLM cost.)

## 6. Reproducibility & cache tests (AC-RDM-08, AC-RDM-09)

**R-ART-17** Rebuilding with no source change makes zero provider calls (AC-RDM-08).
**R-ART-18** Same source + input + lock + seed gives the same result on any machine holding the committed artifacts,
with no network (AC-RDM-09, INV-3, INV-7).

## 7. Production store (Future)

Hosted services may put a hot index in Redis, artifacts and debug metadata in object storage and project/version
metadata in PostgreSQL. Redis is never the source of truth; the content address and the formats in §3 and §5 do not
change, so local and hosted stores are interchangeable.

## 8. Coach (Future)

Background optimization that proposes a better artifact for an existing goal.

```
local telemetry → slow goal detected → candidate queued → new IR synthesized
  → validate → full verification → differential equivalence → benchmark → proposed lock change
```

**R-ART-19** Optimization safety rule: a candidate is acceptable only if it has the same `contract_key` (same
signature, checks, examples), passes full verification, and produces identical outputs and outcome codes to the
current artifact on the whole verification input set. It must improve a metric (fuel, memory, artifact size) without
regressing correctness; latency alone is never enough.
**R-ART-20** Coach never hot-swaps: it proposes a lock change that the user accepts like any other diff. A/B testing,
traffic sampling, automatic promotion and rollback belong to hosted services, later.

## 9. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-ART-01 | Building a golden project twice: the second build makes zero provider calls and leaves `velme.lock` and `.velme/artifacts/` byte-identical — AC-RDM-08. |
| AC-ART-02 | Changing only `CalculateScore`'s plan and rebuilding re-synthesizes `CalculateScore` only; `BuildPlayerSummary`'s lock entry is unchanged. |
| AC-ART-03 | Changing the configured model and rebuilding with an up-to-date lock makes zero provider calls. |
| AC-ART-04 | Adding a field to `Player` marks every goal whose signature reaches `Player` stale, with a cause naming `Player`. |
| AC-ART-05 | `--locked` with one stale goal fails with `VL0702` listing it; no provider is constructed. |
| AC-ART-06 | A tampered artifact file fails with `VL0703`; a deleted one with `VL0701`. |
| AC-ART-07 | Cloning a built project to another OS and running offline gives the same result and trace — AC-RDM-09. One test process cannot show this, so a built fixture project (`velme.lock` and its artifacts) is committed with a golden trace, and CI runs `velme run --locked --offline` on it on every OS of its matrix and compares the trace, durations excluded (D-107). |
| AC-ART-08 | Artifact documents contain no timestamps, usernames or hostnames (schema test). |
| AC-ART-09 | A wired goal gets a lock entry and an artifact with `"provider": "compiler"`. |
| AC-ART-10 | Regenerating `FindBadge` so its behaviour changes, then rebuilding `BuildPlayerSummary`: `BuildPlayerSummary` is re-verified against the new `FindBadge` with no provider call; if it now fails a check, it is re-synthesized with a diagnostic naming `FindBadge` (D-55). |
| AC-ART-11 | Editing a locked goal's IR to break one of its own `examples:` while keeping a hash-consistent artifact file: `velme test --locked` fails on that example (D-46); `velme run` with the same artifact does not detect it (no example re-run). |
| AC-ART-12 | A lock entry whose `contract_key` differs from its artifact's manifest `contract_key` is stale with `VL0702`, even though the artifact file's own hash still matches (D-46). |
