# Contributing to Velme

Thanks for helping. Velme is in the **specification phase**: the spec in `docs/spec/` is complete for v0.1 and code
starts at plan phase M0 (`docs/plan/mvp-plan.md`). Spec feedback, questions and typo fixes are welcome now; code
contributions open once the workspace exists.

Questions and language ideas go to GitHub Discussions; bugs and tasks to Issues (`delivery/52` §8).

---

## Finding your way around the spec

`docs/spec/` is the source of truth. You don't need to read all of it.

- Start at **`docs/spec/SPEC.md`**: its §1 routing table tells you which child file covers your task, and §2 lists
  the invariants `INV-1..10`. The first five lines of each child file say what it covers.
- Every rule has a stable id: `R-*` rules, `AC-*` acceptance criteria, `INV-*` invariants, `D-*` decisions, `Q-*` open
  questions. Search for one with `grep -rn 'R-GOAL-04' docs/spec`.
- The *why* behind a rule is in `reference/92` (`D-n`).
- **Cite ids, don't restate the spec** in code comments, commit messages or new docs (`R-IR-07`, `AC-RUN-03`, `INV-3`,
  `D-9`).
- If code and spec disagree, or the spec is silent, open an issue rather than picking one.

## Non-negotiables

- **Invariants `INV-1..10`** (SPEC.md §2). Breaking one is an architecture decision, never a patch: raise it in an
  issue or RFC first. The ones code most often touches: the IR validator is the trust boundary (INV-1); checks are
  never LLM-judged (INV-2); interpreter and WASM give identical results (INV-3); no ambient authority (INV-4);
  everything is budgeted (INV-5); synthesized IR never adds calls (INV-6, D-5).
- **Crate boundaries** (`compiler/20`, INV-9): `syntax → sema → ir → (interp, wasm) → runtime → cli`. Core crates never
  depend on `velme-cli` or on a concrete provider; vendor SDK types stay inside `velme-synth`'s provider module.
- **Determinism**: no `HashMap` iteration order, wall-clock time, thread scheduling or unseeded randomness may reach a
  result, a trace, a fingerprint or a diagnostic ordering. Use `BTreeMap`/`IndexMap` or sort explicitly.
- **Diagnostic codes** (`reference/90`): a code is never reused or renumbered (INV-10). A new code is added there
  first.
- **Status tags**: build `v0.1`; for `v0.1-ready` only reserve the keyword/field/schema slot; never build `Future`.

## Writing code

- **Match surrounding code**: naming, comment density, idiom. No speculative abstractions, no stubs for later phases.
- **Conventions** are in `docs/code-conventions.md` (`CC-*`); read it before your first PR. In particular, diagnostic
  codes, keywords, builtin names, IR node tags and version strings are defined once in their owning crate and
  referenced by name (CC-CONST-*), and there is no `unwrap`/`expect`/`panic!` on input-derived data (CC-ERR-*).
- **Comments explain *why*,** at the line they guard, once and tightly.
- **Tests** follow `delivery/51`: test names carry the criterion id (`ac_goal_04_self_call_is_rejected`); parser,
  diagnostics and IR outputs are golden snapshots (`insta`); every language rule gets a rejecting test with the
  expected `VLnnnn`; interpreter/WASM parity is a differential test; **never call a live LLM from a test**. Use the
  `scripted` or `replay` provider (D-13).
- **Snapshot changes are reviewed, not blessed blindly**: after `cargo insta review`, say in the PR which snapshots
  changed and why.

## Commands

Available from M0 (`delivery/51` §4, `tooling/40`):

| What | Command |
|---|---|
| Full gate (fmt, clippy `-D warnings`, tests + snapshots, docs, cargo-deny, layering, AC audit) | `cargo xtask verify` |
| Inner loop | `cargo xtask verify --quick`, or `cargo test -p <crate> [filter]` |
| Snapshots | `cargo insta test -p <crate>`, then `cargo insta review` |
| CLI | `cargo run -p velme-cli -- check examples/beginner/hello.velme` |
| Live LLM evidence (by hand, costs money, never in the gate) | `velme build` against `anthropic` in a scratch directory, once per synthesis criterion (D-148) |

Install the snapshot and dependency tools once: `cargo install cargo-insta cargo-deny --locked`.

Every change keeps `cargo xtask verify` green (R-QA-07).

## Branches, commits and PRs

Git flow is R-REL-12 (`delivery/52` §4). In short:

- Never commit to `main`; work on a branch (`m1-syntax`, `spec/<topic>`, `fix/<topic>`).
- Each commit is green on its own. Messages are Conventional Commits with the crate as scope and the ids covered:
  `feat(syntax): indentation and block scalars (AC-SYN-03..06)`; spec edits use `docs(spec)`.
- Snapshots, `Cargo.lock` and replay fixtures are committed with the code that changed them.
- PRs are rebase-merged after one approving review with the gate green (R-REL-05).

## Changing the spec or the language

- **Spec edits**: change the owning file only, keep ids stable (never renumber; retire an id by marking it
  `Retired`), update `reference/92` if affected, and list the ids you touched in the PR.
- **New design choices** are recorded as a `D-n` in `reference/92`, or a `Q-n` while still open.
- **Syntax or semantics changes** need an accepted RFC first (`delivery/52` §9, R-REL-11): Discussion → RFC PR
  (`rfc/NNNN-title.md`) → accepted → implementation, tests and spec update together.

## Using AI assistants

AI-assisted contributions are welcome. You are the author: you must understand every line you submit, be able to
explain it in review, and have run the gate yourself. Say in the PR if a substantial part was generated.

Everything that applies to a contributor is in this file and in `docs/spec/` — there is nothing an assistant needs that
a human does not.

## License

By contributing you agree that your code is licensed `MIT OR Apache-2.0` and your spec/docs contributions CC BY 4.0
(`delivery/52` §7, D-38).
