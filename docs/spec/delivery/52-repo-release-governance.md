# 52 — Repository, Release & Governance

**Status:** v0.1 · **Area:** REL
**Read when:** changing repo layout, adding a crate, editing CI or release workflows, versioning, publishing, licensing,
or proposing a language change (RFC).
**Depends on:** [SPEC](../SPEC.md), [51-testing-quality](51-testing-quality.md), [41-security-privacy](../tooling/41-security-privacy.md)
**Source:** §42A, §43.1–§43.15

## 1. Purpose & boundaries

Defines where code lives, how dependencies may flow, how changes are reviewed and released, and how the language
evolves. Velme is open core (P-7): language, compiler, runtime, CLI, spec and examples are public; hosted services are
separate private repositories.

## 2. Repository layout (v0.1, D-15)

```
velme/
├── Cargo.toml  Cargo.lock  rust-toolchain.toml  rustfmt.toml  deny.toml
├── README.md  LICENSE-MIT  LICENSE-APACHE  LICENSE-CC-BY  CONTRIBUTING.md  CODE_OF_CONDUCT.md  SECURITY.md  CHANGELOG.md
├── AGENTS.md  CLAUDE.md          agent instructions (D-40)
├── crates/
│   ├── velme-syntax/  velme-diagnostics/  velme-sema/  velme-ir/  velme-check/  velme-builtins/
│   ├── velme-interp/  velme-synth/  velme-runtime/  velme-wasm/  velme-cli/  velme-test-support/
├── examples/{beginner,intermediate,games,professional}/   each with velme.lock + .velme/artifacts
├── tests/
│   ├── golden/{parser,diagnostics,ir,explain,trace}/
│   └── fixtures/synth/           replay provider fixtures
├── benches/                      criterion benchmarks (or per-crate benches/)
├── fuzz/                         cargo-fuzz targets: parser, ir_validator, runtime
├── xtask/                        verify, ac-audit, layering check, release helpers
├── rfc/                          README.md, 0000-template.md, accepted RFCs
├── docs/{spec,plan}/  docs/code-conventions.md
└── .github/{workflows/,ISSUE_TEMPLATE/,PULL_REQUEST_TEMPLATE.md,CODEOWNERS,dependabot.yml,release.yml}
```

**R-REL-01** A new crate needs a real API or ownership boundary and an update to SPEC.md §5 and INV-9's order; a
directory alone is never a reason (§43.4).
**R-REL-02** Grows later, not in v0.1: `language/spec/0.1/` (published, versioned mirror of the language chapters of
`docs/spec` — created at the first release), `editors/{tree-sitter,vscode}/` (post-MVP, D-25), `stdlib/` (when goals
can be imported, Future).

## 3. Dependency direction (INV-9)

Layer order: `syntax → sema → ir → (interp, wasm) → runtime → cli`. The exact crate-to-crate edges (where
`velme-diagnostics`, `velme-builtins`, `velme-check` and `velme-synth` sit) are owned by `compiler/20`; this file owns
their enforcement. `velme-test-support` is a dev-dependency only.

**R-REL-03** `xtask layering` reads `cargo metadata` and fails if any crate depends on one to its right or on
`velme-cli`; `velme-synth` is the only crate allowed an HTTP client; no core crate depends on a vendor SDK (INV-7).
`deny.toml` bans duplicate/unsafe sources as a second line.
**R-REL-04** Provider implementations live behind cargo features of `velme-synth` (`provider-anthropic`,
`provider-ollama` and `provider-external`, all default on in the CLI); core crates compile with no provider feature
enabled.

## 4. Branches, reviews and CI (§43.6, §43.7)

**R-REL-05** `main` is protected: changes land by pull request with green required checks and one approving review;
no force-push. `CODEOWNERS` covers `docs/spec/`, `crates/velme-ir/`, `crates/velme-sema/`, `crates/velme-runtime/`,
`crates/velme-wasm/`, `crates/velme-synth/`, `xtask/`, `.github/workflows/`, `deny.toml`, `rust-toolchain.toml`,
`Cargo.lock`.

| Workflow | Trigger | Runs |
|---|---|---|
| `ci.yml` | every PR and push to `main` | `cargo xtask verify` on Linux; test job on macOS + Windows; fuzz smoke; `cargo check --all-targets` on MSRV |
| `nightly.yml` | schedule | long fuzz runs, benchmark suite vs baseline, large examples, cross-platform determinism |
| `live-llm.yml` | manual dispatch only | live provider tests with repository secret; never on forks' PRs |
| `release.yml` | tag `vX.Y.Z[-pre]` | full verify → build Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows x86_64 → release tests → reproducibility check → checksums + signatures → GitHub Release → `cargo publish` |

**R-REL-12** Git flow (trunk-based; applies now, before CI exists):
- `main` is always green (`cargo xtask verify` passes) and only moves by merging a branch — never direct commits.
- One branch per plan phase (`m1-syntax`) or spec change (`spec/<topic>`); it lives until its gate is approved.
- One commit per slice, green on its own so `git bisect` works. Message: Conventional Commits with the crate as
  scope and the ids covered, e.g. `feat(syntax): indentation and block scalars (AC-SYN-03..06)`; spec edits use
  `docs(spec)`. Snapshots, `Cargo.lock` and replay fixtures are committed with the code that changed them.
- At the Stop & Verify Gate: rebase on `main`, full verify, open a PR (`gh pr create`) whose body is the gate report,
  merge with `--rebase` after approval (slice commits kept, no merge bubbles), tag `mN-verified`, record the SHA in
  the plan's gate log, delete the branch.
- Until `ci.yml` exists, the required check is the local `cargo xtask verify`; review approval is the user's gate
  approval. R-REL-05 applies in full once there is a second contributor.

**R-REL-06** CI uses the pinned toolchain from `rust-toolchain.toml`; `Cargo.lock` is committed; workflows pin actions
by commit SHA.

## 5. Versions and release channels (§43.8, §48)

| Version | Example | Bumped when |
|---|---|---|
| Language | `velme/0.1` | syntax or semantics change (RFC) |
| Compiler/CLI | `0.1.4` (semver) | any release |
| IR | `ir/0.2` | IR schema changes (`compiler/21`) |
| Builtins | `builtins/0.1` | builtin added/changed (`language/14`) |
| Prompt | `prompt/3` | prompt template changes (`compiler/22`) |

**R-REL-07** Versions are independent and all recorded in every artifact manifest (INV-8). Any bump that changes a
fingerprint input is noted in `CHANGELOG.md` with "invalidates locks: yes/no".
**R-REL-08** Channels: `nightly` (from `main`, unsigned tags), `alpha`/`beta` (pre-release tags), `stable`. Breaking
changes before 1.0 bump the minor version; the CLI exit codes (R-CLI-10), diagnostic codes (INV-10) and `--json`
schema count as public API.

## 6. Distribution and docs (§43.9, §43.10)

| Channel | When | Source of truth |
|---|---|---|
| GitHub Releases (binaries + checksums + signatures) | v0.1 | release workflow |
| crates.io (`velme-cli`, library crates) | v0.1 | same tag |
| `cargo install velme-cli` | v0.1 | crates.io |
| Homebrew, winget, Scoop, npm installer, OCI image | later | downstream of GitHub Releases only |

Docs live in `docs/` in this repo during the MVP and are published from the same tagged source. Planned hierarchy:
Learn (First Goal, Types, Calls, Checks) · Language Reference · Standard Library · Compiler · Runtime · Security ·
RFCs · Contributor Guide. The Learn track is written for children; the reference is precise enough for professionals.

## 7. Licensing and open core (§42A, D-38)

| Asset | License |
|---|---|
| Source code | `MIT OR Apache-2.0` (dual, the Rust ecosystem convention; `license` field in every `Cargo.toml`) |
| Language spec, docs, examples' prose | CC BY 4.0 |
| Name "Velme" and logo | trademark, not licensed by the above |

Commercial boundary: open — language, compiler, runtime, CLI, WASM backend, VibeVM, spec, examples; hosted — Velme
Cloud (hosted Spellbook, model routing, Coach service, artifact service, telemetry, collaboration, deployment).
**R-REL-09** The open distribution never requires a Velme account or hosted service (INV-7, §42A.3); the moat is
operations and developer experience, not hidden semantics. Legal review of license and trademark before public
commercial launch (D-38).

## 8. Repositories (§43.2, §43.12, §43.13)

| Repo | Visibility | When |
|---|---|---|
| `velme-lang/velme` | public | now — compiler, runtime, CLI, spec, examples, tests, docs |
| `velme-lang/velme-vscode` | public | when editor tooling is substantial |
| `velme-lang/velme-website` | public | optional; docs stay in main repo first |
| `velme-lang/velme-cloud` | private | when hosted service development starts |
| `velme-lang/velme-console`, `velme-lang/velme-infra` | private | later |

**R-REL-10** The public repo never contains production secrets, infrastructure credentials, customer code or
proprietary optimization datasets. Split a repo only for a different release lifecycle, access model, deployment
target or contributor community.

GitHub usage: Issues for bugs and tasks; Discussions for language ideas, beginner questions, announcements; Projects
for the MVP roadmap and language versions; Releases for binaries.

## 9. Language evolution: RFCs

`idea (Discussion) → RFC PR (rfc/NNNN-title.md from template) → discussion → accepted → implementation + tests →
spec update → released`. Required for: new syntax, semantics changes, new effects/capabilities, IR breaking changes,
new host functions. Reserved words (D-24) mark expected RFC topics: effects, modules, conditional calls.

**R-REL-11** No syntax or semantics change merges without an accepted RFC and a spec update in the same release.

## 10. First commit and README (§43.14)

The first public commit contains: README, both licenses, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, CHANGELOG,
workspace `Cargo.toml`, `rust-toolchain.toml`, `crates/`, `examples/`, `tests/`, `docs/`, `rfc/`, `.github/`.
README order: 1 What is Velme? · 2 Why is it different? · 3 a 10-line example a child can read · 4 run it locally ·
5 the kid → professional path · 6 architecture · 7 contribute · 8 license.

## 11. Naming clearance (D-38)

Screened 2026-09-27 for **Velme** (D-38): registries, org and domains free; USPTO and TMview show no live VELME mark in
classes 9 or 42, near marks low risk. Claimed 2026-09-27: GitHub org `velme-lang` and repo `velme`; `velme` and every
`velme-*` crate in §2 on crates.io at 0.0.0 (owners: the maintainer and `github:velme-lang:owners`); npm org `velme`,
owning the `@velme/*` scope (npm refuses the bare name `velme` as too close to existing packages, so npm packages are
scoped); PyPI `velme` at 0.0.0; domain `velme.dev`. Still owed before filing a mark: an attorney's clearance opinion,
then filings in the US, EU and target markets.

## 12. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-REL-01 | The workspace builds with exactly the D-15 crates and `xtask`; `rust-toolchain.toml` pins the toolchain. |
| AC-REL-02 | `xtask layering` fails when a test crate edge violating INV-9 is added (e.g. `velme-syntax → velme-runtime`). |
| AC-REL-03 | Core crates (all but `velme-cli`) build with `--no-default-features` and no provider SDK in their dependency tree. |
| AC-REL-04 | A release dry run produces binaries for the five targets, checksums and signatures, and two builds of the same tag have identical checksums. |
| AC-REL-05 | Every artifact manifest records language, compiler, IR, builtins, prompt and model versions. |
