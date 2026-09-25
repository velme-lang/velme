# CLAUDE.md

@CONTRIBUTING.md
@AGENTS.md

---

## Claude Code specifics

**Reviews before the phase gate.** Run them **sequentially, each fully fixed before the next starts**, never
concurrently or interleaved:

1. the phase's `architect-review` (Opus), if its "Model" line requires one (language semantics / `INV-*` / crate
   boundary / sandbox or security);
2. `/code-review` (medium) on the diff, now including the architect-review fixes;
3. `/simplify` on the diff, last, once the code is settled.

Fix findings at each step, or list the ones you're deliberately leaving and why, before moving to the next step.

**Per slice** (AGENTS.md §Slices): the slice review is `/code-review` (medium); use `/compact` or a fresh session
between slices; skip `/simplify` per slice.

## Subagents

Default to doing the work inline: a spawned agent starts cold and must be re-briefed, which usually costs more than
it saves. Spawn only for genuinely independent or large work, at most 3 concurrently, and pick the cheapest tier that
fits (`.claude/agents/`):

- **`mechanical-task`** (Haiku, effort low): fully specified, no judgment. Snapshot/format checks, renames across a
  given file list, adding a listed diagnostic code everywhere it's named.
- **`routine-dev`** (Sonnet, effort medium): scoped implementation, refactors, tests once the design is decided.
- **`architect-review`** (Opus, effort high): language-semantics and IR decisions, `INV-*`/crate-boundary questions,
  sandbox and security review. Read-only; reports.

Brief a subagent with exact file paths and spec ids so it doesn't re-explore. Use the built-in `Explore` agent only
for broad searches where you need the conclusion, not the file contents.
