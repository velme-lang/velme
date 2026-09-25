# AGENTS.md

Instructions for AI coding agents working in this repo. **`CONTRIBUTING.md` applies to you in full**: read it first
(Claude Code imports it via `CLAUDE.md`). This file adds only what is specific to agents.

The language is **Thela** (styled **ThéLa**, from Greek *thélō*, "I want"; D-1). It was renamed Neya → Nela → Thela;
never write "Neya" or "Nela" anywhere.

---

## Reading the spec (token budget)

The spec is deliberately not imported into agent context.

1. **`docs/spec/SPEC.md`**: read once per session, before the first task that writes code or changes a spec. Skip it
   for trivial work (typos, git chores, questions answerable from files already in context).
2. **Route, don't sweep.** Open only the child files SPEC.md §1 routes you to; `head -6` a file to decide before
   reading its body.
3. **Read by section.** For a known id, `grep -rn 'R-GOAL-04\|AC-CHK-' docs/spec` and read that section. Read a file in
   full only when you're implementing most of it.
4. **Don't re-read** a file already in the conversation, and don't read a spec "just in case".

## Workflow

**Current plan:** `docs/plan/mvp-plan.md`. Read its §1 progress tracker plus only the active phase; update the tracker
row and gate log at each Stop & Verify Gate.

1. **Scope.** Work only on the assigned plan phase (`delivery/50`) or task. Don't implement or scaffold future phases.
   If a requirement is ambiguous or the spec is silent, ask one focused question rather than guessing; if the answer
   is a new design choice, record it as a `D-n` (or `Q-n`) in `reference/92`.
2. **Plan when it's non-trivial.** A change spanning several crates, a grammar or IR schema change, a new diagnostic
   family, or anything touching an `INV-*` gets a short plan (files, spec ids covered, tests to add) approved before
   code. Small changes skip this.
3. **Implement in small, verifiable steps**: each step compiles and its tests pass before the next begins.
4. **Stop & Verify Gate.** At the end of each phase or task, stop and report:
   - what was implemented, with the `AC-*` ids now covered (and any still uncovered);
   - commands run and their results (say plainly if something failed or was skipped);
   - which snapshots changed and why;
   - exact steps for the user to verify locally (usually a `thela …` command on a file in `examples/`), and what they
     should see.
5. **Approval required.** Wait for explicit confirmation (e.g. "M1 verified, proceed to M2") before the next phase.
   Commit only when asked; follow R-REL-12 (branch per phase, one green commit per slice, PR + rebase-merge +
   `mN-verified` tag at the gate).

**Invariants:** if a change would break an `INV-*`, flag it and stop. Never patch around one.

## Slices

A phase whose plan body splits it into slices is built one slice at a time. Each slice ends with its tests green, a
review of that slice's diff only, fixes, and a commit before the next slice starts (start from fresh context between
slices). The phase-gate review then focuses on where slices meet rather than re-reviewing each slice.
