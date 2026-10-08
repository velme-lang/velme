# agent-bridge

Dev tooling, not part of Velme. It lets a coding agent stand in for the LLM provider: the bridge is an `external` backend
(`docs/spec/compiler/22-spellbook-synthesis.md` §3.2) that answers from a folder of answers and queues every other request
as `pending` (R-SYNTH-41). One Python 3 script, standard library only, bound to 127.0.0.1.

## Loop

1. Start the bridge (keep it running), from the project directory:

   ```
   python3 tools/agent-bridge/bridge.py --answers answers --pending pending --port 8787
   ```

   `--token T` (or `VELME_EXTERNAL_TOKEN`) makes it require `Authorization: Bearer T`.
2. Build with the external provider:

   ```
   velme build game.velme --provider external --external-url http://127.0.0.1:8787
   ```

   Each goal without an answer fails with `VL0408` and a note `queued as <id>`; the request is `pending/<id>.json`. The
   filename is the id: use it as it is. The file is pretty-printed, so hashing its content does not give the id.
3. Read `pending/<id>.json`: the canonical `SynthRequest` (goal, signature, types, plan, checks, examples, builtins, `reply_format`
   when it is not `ir-json`, and `output_schema`, the schema of the reply in that format: for `compact` a body under `bo`, or a
   question, not the IR schema). To see the prompt a model would get for the same goal, with the same `[synthesis]` options:

   ```
   cargo xtask render-prompt game.velme Double --provider anthropic [--reply-format ir-json|compact] [--schema-in-prompt full] [--max-examples N] [--model ID] [--pretty]
   ```

   It prints the exact request body the `anthropic` (or `ollama`, or `external`) provider sends for the goal's first attempt,
   built by the providers' own code, with no network or key. It does not read `velme.toml`: pass the options the build uses
   as flags, and a flag it does not know is an error. A retry's request (earlier replies and diagnostics) exists only in a
   build, as the `attempts` of a later `pending/<id>.json`. Answer the `messages` (and `system`, `tools` for anthropic) as
   the model would.
4. Write the answer to `answers/<id>.json`, in the format the request names (`reply_format`; none means `ir-json`). Write
   a temporary file in `answers/` (a name not ending in `.json`) and rename it, so the build never reads half an answer
   (a file that is not valid JSON counts as still pending). For `ir-json` the answer is a bare IR expression node (the
   bridge wraps it as `{"body": <node>}`) or `{"body": <node>}`; in any other format it is sent exactly as written
   (`compact`: `{"bo": ...}`). `{"question": "..."}` and `{"error": "..."}` work in every format.
5. Rerun the build. Velme validates and verifies the body like any model's (INV-1); a failed body is the answer's fault,
   not the bridge's. The id is a hash of the request, so an edited goal is queued again under a new id.

## Experiment use (P0e)

Each subagent plays the provider: it gets only the `render-prompt` output for a goal (never the source's other goals or the
repo) and writes the reply a model would, in the reply format under test. The bridge's `describe` lists `ir-json`, `compact` and `notation`, so any `[synthesis] reply_format` is accepted (a
backend that does not list the format stops the build before any request, `VL0406`). The bridge stands between the build and the
agents so the same `velme build` path, validator and verifier judge every answer; the synth log records the build as usual.
