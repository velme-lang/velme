# Examples

Small programs you can read without the spec, grouped by who they're for: `beginner/`, `intermediate/`, `games/` and
`professional/`. Each one is complete and passes `velme check`:

```sh
cargo run -p velme-cli -- check examples/beginner/find_badge.velme
```

For now `velme check` only parses (plan phase M1); later phases add type checking, building and running. Each example
gets its `velme.lock` and `.velme/artifacts` once `velme build` exists (`docs/spec/delivery/52` §2).
