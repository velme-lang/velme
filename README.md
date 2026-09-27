# Velme

**Velme™** (Latvian *vēlme*, "wish") is an intent-driven programming language — **kids first, professional by design**. You declare typed goals,
describe what each should do in plain language, and state checks the result must pass. An LLM proposes an
implementation as typed Velme IR; a deterministic compiler, validator and sandboxed runtime decide whether it is
accepted and run it.

```text
goal Add(a: Number, b: Number) -> Number:
    plan: "Add a and b."

    check:
        - result == a + b
```

## Why not just ask an AI to write the code?

You can, and for many jobs you should. Velme is for when you want the AI's help but not its judgement: you decide
what the program does, and a deterministic compiler decides whether the AI got it right.

| | AI writes the code | Velme |
|---|---|---|
| What you keep and review | code you didn't write, every line of it | the goal: its purpose, its calls and its examples |
| Who decides it works | tests, often written by the same AI | your `check`s and `examples`, run by the runtime, never by an LLM (INV-2) |
| What the AI output can do | anything the language can: files, network, endless loops | only typed IR that passes the validator (INV-1), in a sandbox with no files, network or clock (INV-4) and fixed budgets (INV-5) |
| Program structure | whatever the AI chose | fixed by your `call` blocks; the AI fills in single goals and can't add calls (INV-6) |
| Running it again | new chat, possibly different code | same source, inputs and lockfile give the same result, offline (INV-3, INV-7); an unchanged goal is never re-synthesized (AC-RDM-08) |
| When it fails | read the code to find out why | the failed example or check, with the values, names the goal that is wrong (AC-RDM-06) |

For learners this moves the effort to the parts of programming that stay a person's job: breaking a problem into
goals, saying precisely what each one should do, and giving examples that prove it.

It is not the right tool for everything. v0.1 goals can't read files, use the network, loop or recurse in source
(`delivery/50`), and a one-line rule is no shorter as a plan plus examples than as code plus a test. Velme saves the
most on goals that are quick to describe but long to implement.

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
cargo run -p velme-cli -- --version
```

## License

Code is `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`); the spec and docs are CC BY 4.0
(`LICENSE-CC-BY`). The name and logo are trademarks and not licensed by these.
