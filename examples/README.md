# Examples

Small programs you can read without the spec, grouped by who they're for:

| Folder | Example | What it shows |
|---|---|---|
| `beginner/` | `hello.velme`, `add.velme` | one goal: a plan, a `check` and `examples:` |
| `beginner/` | `find_badge.velme` | a record type and a multi-line plan |
| `beginner/` | `double_then_add_one.velme` | a `call` block joining two goals |
| `intermediate/` | `player_summary.velme` | goals that run at the same time, then a summary |
| `games/` | `level_summary.velme` | six goals building a level summary |
| `professional/` | `order_total.velme` | a pricing rule as subtotal, discount and shipping |

Each folder commits its `velme.lock` and its built goals in `.velme/artifacts`, so every example runs and tests with no
API key and no network. From the repository root:

```sh
velme run examples/beginner/add.velme --goal Add --arg a=2 --arg b=3
velme test --locked examples/games/level_summary.velme
velme explain examples/intermediate/player_summary.velme --goal BuildPlayerSummary
```

`velme test --locked` runs each goal's examples and generated-input checks against the committed artifacts and writes
nothing; the test suite runs it on every example, so an example that stops working fails `cargo xtask verify`.

## How the locks and artifacts are made

They are not written by a live model. The tests build each example on the `replay` provider, from fixtures recorded
from the hand-written IR in `tests/fixtures/run`, and check that the committed files are byte for byte what that build
writes (D-141). Each artifact's manifest records the compiler version, so they are rebuilt whenever it changes, and
whenever an example or its IR changes:

```sh
VELME_BLESS_FIXTURES=1 cargo test -p velme-cli --features velme-cli/test-provider --test examples
```

Then review the diff. A new example also needs its IR in `tests/fixtures/run` and an entry in `EXAMPLES`
(`crates/velme-test-support/src/differential.rs`).
