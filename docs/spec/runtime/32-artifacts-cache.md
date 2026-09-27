# 32 — Artifacts, Fingerprints, Lockfile & Cache

**Status:** v0.1 · **Area:** ART
**Read when:** computing a fingerprint, reading/writing an artifact or `velme.lock`, deciding whether synthesis is needed, or touching the WASM/derived caches or Coach.
**Depends on:** [SPEC](../SPEC.md), [compiler/21](../compiler/21-ir.md), [compiler/22](../compiler/22-spellbook-synthesis.md), [30-execution-vibevm](30-execution-vibevm.md)
**Source:** §24.2, §25, §26, §35, §36, §42 (Cache), §43.8, §48, §52 (Test 8, 9)

## 1. Purpose & boundaries

An **artifact** is a verified IR goal plus the manifest that says how it was produced. Artifacts are content-addressed
and immutable (INV-8). `velme.lock` pins which artifact each goal uses, which is what makes runs reproducible even
though LLM output is not (D-12). This file owns identities, formats and staleness; producing artifacts is
[compiler/22](../compiler/22-spellbook-synthesis.md), running them is [30](30-execution-vibevm.md).

## 2. Identities (§25, D-11, D-21)

All hashes are BLAKE3 over canonical JSON (21 R-IR-21), written `b3:<hex>`.

| Identity | Hash of | Used for |
|---|---|---|
| `signature` | goal name, param names + types, output type, every reachable record type | `Call.goal_signature`; parents' keys |
| `contract_key` | normalized goal source (signature, plan per D-21, `call` bindings, checks, examples, budget) + child `signature`s + `language_version` + `ir_version` major + `builtins_version` | lock staleness (§5); generated-input seed (22 §7) |
| `synthesis_key` | `contract_key` + provider `input_version` (the `prompt_version`, or the external `request_version`) + compiler `MAJOR.MINOR` + provider id + model id (Ollama `<model>@<digest>`, external `backend_version`; `compiler/22` §3) | artifact-store lookup; replay fixture name |
| `artifact` | the canonical artifact document (§3) | store address, lock pin |
| `execution_id` | `artifact` + children's `execution_id`s in binding order | exact tree identity; shown as `artifact_id` in traces (§25) |

**R-ART-01** Nothing is cached, looked up or pinned by goal name alone (INV-8, §25).
**R-ART-02** Child **signatures**, not child artifacts, enter a parent's keys (D-11): regenerating `CalculateScore`
leaves `BuildPlayerSummary`'s keys and artifact unchanged; only its `execution_id` changes.
**R-ART-22** When `velme build` changes a child's artifact, it re-runs every ancestor composite's `examples` and
`check`s against the new children, with no LLM call (D-55). An ancestor that now fails is stale and goes through
ordinary synthesis (`compiler/22`); its diagnostic names the child that changed.
**R-ART-03** A verified artifact stays valid when only the prompt, compiler patch/minor, provider or model changes:
those enter `synthesis_key` (cache reuse) but not `contract_key` (validity). Switching models never forces
re-synthesis of a locked project.
**R-ART-04** Compiler patch versions never enter any key; `compiler_version` is recorded in the manifest only.

## 3. Artifact document & manifest (§43.8, §48)

```json
{
  "manifest": {
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
counts (those go to the local synth log, 22 R-SYNTH-23). Two machines accepting the same IR produce the same `artifact`
hash.
**R-ART-06** `stdlib_version` (§43.8) is `builtins_version` in v0.1 — there is no separate standard library.
**R-ART-07** Wired goals (D-4) produce artifacts too, with `"provider": "compiler"` and no `model_version`, so every
goal resolves through the lock the same way.
**R-ART-21** An `external` artifact records `"provider": "external"`, the backend name, `model_version` =
`backend_version` and `prompt_version` = `request_version`; an `ollama` artifact records `model_version` =
`<model>@<digest>` (D-41, D-42).
**R-ART-08** `children` lists `{binding, goal, signature}` in source order, mirroring the IR `calls`.

## 4. Local store layout (§26 MVP)

```
<project>/
  velme.toml                   config (22 §8)
  velme.lock                   pins — commit
  .velme/
    artifacts/b3-<hex>.json   verified artifacts — commit (D-12)
    cache/wasm/…              derived modules (31 §7) — ignore
    synth-log.jsonl           local attempt log — ignore
```

**R-ART-09** The store is append-only and write-once: a file is written to a temp name and atomically renamed; an
existing file with the same name is never rewritten.
**R-ART-10** Every load re-hashes the file, re-runs IR validation (21 §6, cached per hash for the process), and
cross-checks the manifest's `goal`, `signature` and `contract_key` against the lock entry and against the key computed
from current source (D-46): the manifest's claims are informational, never trusted on their own. A hash mismatch is
`VL0703 ArtifactCorrupt`; a missing file is `VL0701 ArtifactUnavailable`; a manifest/lock/computed mismatch or a
re-validation failure makes the entry stale (R-ART-14).
**R-ART-11** Only candidates that passed the full verification pipeline (22 §6) are written (§22). A failed or
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
disagreeing with the lock entry or fails re-validation (D-46). The CLI names the cause by comparing key components
("the plan changed", "`Player` gained a field", "built for language 0.1, file says 0.2", "the stored artifact doesn't
match its own lock entry").
**R-ART-15** `velme build` re-synthesizes only stale or missing goals (post-order, 22 R-SYNTH-01), updates their entries
and prunes entries for deleted goals. `--locked` performs no synthesis and fails with `VL0702 LockStale` listing every
stale goal and its cause. CI runs with `--locked`.
**R-ART-16** `velme run`/`velme test` never synthesize implicitly unless the user passes `--build`; by default a stale or
missing entry is `VL0702`/`VL0701` with a hint to run `velme build`. (Keeps "run" free of surprise LLM cost.)

## 6. Reproducibility & cache tests (§52 Tests 8–9)

**R-ART-17** Rebuilding with no source change makes zero provider calls (§52 Test 8).
**R-ART-18** Same source + input + lock + seed gives the same result on any machine holding the committed artifacts,
with no network (§52 Test 9, INV-3, INV-7).

## 7. Production store (Future, §26)

Hosted services may put a hot index in Redis, artifacts and debug metadata in object storage and project/version
metadata in PostgreSQL. Redis is never the source of truth; the content address and the formats in §3 and §5 do not
change, so local and hosted stores are interchangeable.

## 8. Coach (Future, §35–§36)

Background optimization that proposes a better artifact for an existing goal.

```
local telemetry → slow goal detected → candidate queued → new IR synthesized
  → validate → full verification → differential equivalence → benchmark → proposed lock change
```

**R-ART-19** Optimization safety rule (§36): a candidate is acceptable only if it has the same `contract_key` (same
signature, checks, examples), passes full verification, and produces identical outputs and outcome codes to the
current artifact on the whole verification input set. It must improve a metric (fuel, memory, artifact size) without
regressing correctness; latency alone is never enough.
**R-ART-20** Coach never hot-swaps: it proposes a lock change that the user accepts like any other diff. A/B testing,
traffic sampling, automatic promotion and rollback belong to hosted services, later.

## 9. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-ART-01 | Building a golden project twice: the second build makes zero provider calls and leaves `velme.lock` and `.velme/artifacts/` byte-identical — §52 Test 8. |
| AC-ART-02 | Changing only `CalculateScore`'s plan and rebuilding re-synthesizes `CalculateScore` only; `BuildPlayerSummary`'s lock entry is unchanged. |
| AC-ART-03 | Changing the configured model and rebuilding with an up-to-date lock makes zero provider calls. |
| AC-ART-04 | Adding a field to `Player` marks every goal whose signature reaches `Player` stale, with a cause naming `Player`. |
| AC-ART-05 | `--locked` with one stale goal fails with `VL0702` listing it; no provider is constructed. |
| AC-ART-06 | A tampered artifact file fails with `VL0703`; a deleted one with `VL0701`. |
| AC-ART-07 | Cloning a built project to another OS and running offline gives the same result and trace — §52 Test 9. |
| AC-ART-08 | Artifact documents contain no timestamps, usernames or hostnames (schema test). |
| AC-ART-09 | A wired goal gets a lock entry and an artifact with `"provider": "compiler"`. |
| AC-ART-10 | Regenerating `FindBadge` so its behaviour changes, then rebuilding `BuildPlayerSummary`: `BuildPlayerSummary` is re-verified against the new `FindBadge` with no provider call; if it now fails a check, it is re-synthesized with a diagnostic naming `FindBadge` (D-55). |
| AC-ART-11 | Editing a locked goal's IR to break one of its own `examples:` while keeping a hash-consistent artifact file: `velme test --locked` fails on that example (D-46); `velme run` with the same artifact does not detect it (no example re-run). |
| AC-ART-12 | A lock entry whose `contract_key` differs from its artifact's manifest `contract_key` is stale with `VL0702`, even though the artifact file's own hash still matches (D-46). |
