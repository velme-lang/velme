# Examples

Small programs you can read without the spec, grouped by who they're for: `beginner/`, `intermediate/`, `games/` and
`professional/`. Each one is complete and passes `velme check`:

```sh
cargo run -p velme-cli -- check examples/beginner/find_badge.velme
```

For now `velme check` checks names, types and calls (plan phase M2); later phases add building and running. Each example
gets its `velme.lock` and `.velme/artifacts` once `velme build` exists (`docs/spec/delivery/52` §2).
