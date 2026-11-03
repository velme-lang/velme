# P1 harness: Velme vs Python on a program corpus

One folder per program under `benchmark/p1/<id>/`. A program's `program.json` names its goal, `.velme` file, B1/B2
entry function and input keys, output fields with their comparison (`exact`, or `decimals` with a digit count), and its
oracle (`drools` for DMN, with the model/TCK paths in the descriptor; `python` otherwise, where `<id>/oracle.py` exports
`decide(case)`, and `oracle.py` diffs each row against `<id>/reference.py` when the folder has one). Everything
program-specific lives in that folder.

Arms (same requirements and examples, same model, same retry budget):
- **Velme:** `velme build` the program's `.velme`, then `velme run --goal <goal>` on every held-out input.
- **B1, one-shot:** one request; the B2 prompt without the tests instruction, one `solution.py`, no feedback.
- **B2, Python plus tests:** writes `solution.py` and pytest tests, runs them, and gets failures back for up to 3 fixes.

Run order (from `benchmark/p1`, Python 3.13, `.venv`, docker only for a DMN oracle):

    ./fetch_tck.sh                                     # DMN + TCK cases into .cache/tck (pinned commit)
    python oracle.py --program 0087 --start            # docker Kogito JIT runner
    python oracle.py --program 0087 --selftest         # the TCK's own cases must match
    python gen_inputs.py --program 0087                # inputs.jsonl + kinds.jsonl (commit both)
    python oracle.py --program 0087                    # expected.jsonl (commit it); also diffs vs reference.py
    export ANTHROPIC_API_KEY=...
    python run_velme.py --program 0087 --model ID --trials 5 --out runs
    python run_b1.py    --program 0087 --model ID --trials 5 --out runs
    python run_b2.py    --program 0087 --model ID --trials 5 --out runs
    python score.py runs                                # tables + runs/report.md; oracle.py --program 0087 --stop when done

Programs: `0087` (loan origination), `0020` (vacation days), `0014` (loan comparison: lists, ranking, `pow`); the
three are DMN, so each runs the steps above with its own `--program` (one `--start`/`--stop` serves them all).
`0020` and `0014` get their `heldout/` (inputs, kinds, expected) only once the owner has read their requirements.md.
Our own programs, `order-pricing`, `vat-shipping`, `insurance-quote`, `invoice-validation` and `refund-policy`, use a
Python oracle written from requirements.md alone by a different model family than the arms (not yet written for the
last two); their `heldout/` also waits for the owner's read of requirements.md. Their `reference.py` is ours, a
cross-check only.
`0014`'s outputs are lists of records, scored in order field by field (`compare: list`), payment and equity to 8
places; its TCK case lists double-precision values, so its adapter's `tck_equal` compares the selftest to 8 places.
`invoice-validation`'s `reasons` is a list of codes, scored in order as values (`compare: list` with one `item` spec).

Run layout: every runner takes the run root (`--out runs`) and writes
`runs/<program>/<arm>/<model>/trial-N/{results.jsonl,meta.json}`. `meta.json` records program, arm, model, calls,
`tokens_in`, `tokens_out`, `cache_read`, `cache_write`, wall time and `build_ok` (Velme build; B2's final pytest; B1:
a `solution.py` was extracted and imports). A failed Velme build scores 0/N for that trial, not a missing trial.

Held-out inputs are seeded and weighted to the rules' boundaries. `kinds.jsonl` marks each id `boundary` or `random`,
and every program except 0087 must carry one; 0087's committed inputs carry the old `tags` and their kind is derived
from those (`["random"]` is random, anything else boundary), so its `inputs.jsonl`/`expected.jsonl` stay
byte-identical. An offline test regenerates 0087 with seed 1 into a temp dir and asserts the bytes are unchanged.

Prompt caching is on for every arm (`--no-prompt-cache` turns it off). Velme's synth log is read for
`cache_read_tokens`/`cache_write_tokens`; its `tokens_in` is the provider's `input_tokens`, which excludes cached
tokens, and the B arms read the same `input_tokens` field, so all arms count uncached input in `tokens_in` and cache
apart. `score.py` reports cost as billed (cache reads 0.1×, writes 1.25× input) beside `cost_uncached_usd` (every
input token at the plain input price), from the one price table in `common.PRICES` (P1e confirms the prices).

Scoring pools each model over all programs and trials, and also reports per program: pass rate overall, boundary and
random, float failures in their own column, calls, tokens, wall time, failed builds, cached and uncached cost. The
go/no-go line per model is Velme's pass rate ≥ B2's and cost ≤ 1.5× B2's, or equal pass rates with fewer boundary
failures; if the arms cover different (program, trial) sets the line says INCOMPLETE and lists what is missing.

Offline tests: `python -m pytest -q tests` (no network, docker or `velme` binary). The toy fixtures under
`tests/fixtures/` prove the harness is not 0087-bound.
