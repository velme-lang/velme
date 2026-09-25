# Thela

**ThéLa** (Greek *thélō*, "I want") is an intent-driven programming language — **kids first, professional by design**. You declare typed goals,
describe what each should do in plain language, and state checks the result must pass. An LLM proposes an
implementation as typed Thela IR; a deterministic compiler, validator and sandboxed runtime decide whether it is
accepted and run it.

```text
goal Add(a: Number, b: Number) -> Number:
    plan: "Add a and b."

    check:
        - result == a + b
```

**Status:** specification complete for v0.1; implementation is at plan phase M0 (workspace and quality gate).

- Specification: [`docs/spec/SPEC.md`](docs/spec/SPEC.md)
- Design decisions and open questions: [`docs/spec/reference/92-decisions-questions.md`](docs/spec/reference/92-decisions-questions.md)
- MVP plan: [`docs/plan/mvp-plan.md`](docs/plan/mvp-plan.md)

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). `AGENTS.md`, `CLAUDE.md` and `.claude/` are instructions for AI coding
agents; you can ignore them if you don't use one.

## Build

```sh
cargo xtask verify            # the full quality gate
cargo run -p thela-cli -- --version
```

## License

Code is `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`); the spec and docs are CC BY 4.0
(`LICENSE-CC-BY`). The name and logo are trademarks and not licensed by these.
