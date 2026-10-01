# Examples

Small programs you can read without the spec, grouped by who they're for: `beginner/`, `intermediate/`, `games/` and
`professional/`. Each one is complete and passes `velme check`:

```sh
cargo run -p velme-cli -- check examples/beginner/find_badge.velme
```

`velme build` asks a model to write each goal that has a `plan`, and `velme run` and `velme test` then run it. Before
the first release each example gets its `velme.lock` and `.velme/artifacts`, so it runs with no API key
(`docs/spec/delivery/52` §2).
