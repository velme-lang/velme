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

**Status:** v0.1 is in its last phase (M8, the MVP gate). `velme` checks, builds, tests, runs, traces and explains
programs, on a reference interpreter or a sandboxed WASM backend. Nothing is on crates.io and there is no release yet;
the quick start below installs from this repository. To hear about the first release, watch this repo (Watch → Custom →
Releases); progress notes are posted in [Discussions → Announcements](https://github.com/velme-lang/velme/discussions/categories/announcements).

- Specification: [`docs/spec/SPEC.md`](docs/spec/SPEC.md)
- Design decisions and open questions: [`docs/spec/reference/92-decisions-questions.md`](docs/spec/reference/92-decisions-questions.md)
- MVP plan: [`docs/plan/mvp-plan.md`](docs/plan/mvp-plan.md)

## Run it locally

You need a Rust toolchain ([rustup](https://rustup.rs), Rust 1.98 or later). Install the `velme` command from this
repository, then clone it for the examples:

```sh
cargo install --git https://github.com/velme-lang/velme velme-cli --locked
git clone https://github.com/velme-lang/velme && cd velme
```

Every example comes with its `velme.lock` and its built goals in `.velme/artifacts`, so it runs with no API key and
no network:

```sh
$ velme run examples/beginner/add.velme --goal Add --arg a=2 --arg b=3
Add  ✓

Result:
5
$ velme test examples/beginner/add.velme
Add  ✓ 3 examples, 61 generated inputs
```

To have a model write goals of your own, copy an example to a new folder, change its plan and examples, and build it
with `ANTHROPIC_API_KEY` and a model id set, or with [Ollama](https://ollama.com) running and `--provider ollama`.
Both providers need a model: pass `--model <model-id>` or set `VELME_MODEL`.

```sh
mkdir my-goals && cp examples/beginner/hello.velme my-goals/
export ANTHROPIC_API_KEY=...        # or use --provider ollama instead of the key
export VELME_MODEL=<model-id>       # or pass --model <model-id> to velme build
velme build my-goals/hello.velme    # writes my-goals/velme.lock and my-goals/.velme/
velme run my-goals/hello.velme --goal SayHello --arg 'name="Mia"'
```

`velme build` sends your plans, types, checks and examples to the provider and says so before it does. A second build
of an unchanged goal makes no provider call. [`examples/README.md`](examples/README.md) lists the examples.

## From a first goal to a real program

One language from the first lesson to production code: each step adds to what the one before taught, nothing is
thrown away.

1. **First goal** ([`examples/beginner`](examples/beginner)): one goal, a plan in plain words, a `check` that must
   always hold and an `examples:` list. This is already the whole loop: say what you want, and prove it.
2. **Your own types and several goals** ([`examples/intermediate`](examples/intermediate)): records like `Player`, and
   a `call` block that wires small goals into a bigger one. Goals that don't depend on each other run at the same time.
3. **Games** ([`examples/games`](examples/games)): a level summary built from six goals, where the structure is yours
   and each goal is small enough to check.
4. **Professional** ([`examples/professional`](examples/professional)): the same building blocks for business rules,
   with the lockfile committed and `velme test --locked` in CI, so a build is reviewed once and then reproduced offline.

## Architecture

```text
 source.velme ─▶ syntax ─▶ sema ─▶ IR ◀── synth (LLM provider: anthropic, ollama, external, replay)
                (parse)   (types,   │      proposes IR for each goal with a plan; the validator and
                           calls)   │      the goal's checks and examples decide whether it is accepted
                                    ▼
                    store + velme.lock (content-addressed artifacts, committed)
                                    │
                                    ▼
             runtime: scheduler and budgets ─▶ interpreter or WASM sandbox (no files, network or clock)
```

The pipeline is a set of Rust crates under [`crates/`](crates), with dependencies in one direction only (INV-9):
`velme-syntax` → `velme-sema` → `velme-ir` → `velme-interp` / `velme-wasm` → `velme-runtime` → `velme-cli`, with
`velme-check` and `velme-synth` between the interpreter and their users, and `velme-diagnostics` and `velme-builtins`
usable by any layer. No core crate depends on the CLI or on a concrete LLM provider. An LLM never produces executable
code, only IR that the validator must accept, and never decides whether a result passes: the runtime does (INV-1,
INV-2). The full component list is in
[`docs/spec/SPEC.md`](docs/spec/SPEC.md) §5.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Build

```sh
cargo xtask verify            # the full quality gate
cargo run -p velme-cli -- --version
```

## License

Code is `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`); the spec and docs are CC BY 4.0
(`LICENSE-CC-BY`). The name and logo are trademarks and not licensed by these.
