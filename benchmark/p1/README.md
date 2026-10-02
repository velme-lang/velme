# P1 pilot: Velme vs Python + tests, DMN TCK 0087 (loan origination)
Same plain-English rules (`0087/requirements.md`), same LLM, same retry budget, scored on 200 held-out inputs
against an independent oracle (the original DMN model run by Drools, not written by either arm).
- **Velme arm:** `0087/loan.velme` (plan text + examples) -> `velme build` (up to 3 retries) -> `velme run --goal DecideLoan`.
- **B2 arm:** the model gets requirements + `signatures.py` + `examples.json`, writes `solution.py` and pytest tests,
  pytest output is fed back for up to 3 fix rounds (at most 4 calls).

Run order (from `benchmark/p1`, Python 3.13, `pip install -r requirements.txt`; docker for the oracle only):

    ./fetch_tck.sh                                  # DMN + TCK test cases into .cache/tck (pinned commit, not vendored)
    python oracle.py --start                        # docker run -d --rm -p 8080:8080 quay.io/kiegroup/kogito-jit-runner
    python oracle.py --selftest                     # the TCK's own cases must match through the container
    python gen_inputs.py                            # seed 1 -> 0087/heldout/inputs.jsonl (commit it)
    python oracle.py                                # -> 0087/heldout/expected.jsonl (commit it); also diffs vs reference.py
    export ANTHROPIC_API_KEY=...                    # Velme also reads VELME_API_KEY
    python run_velme.py --model ID --trials 5 --out runs/velme   # needs target/release/velme (cargo build --release -p velme-cli)
    python run_b2.py    --model ID --trials 5 --out runs/b2
    python score.py runs/velme runs/b2              # tables + runs/report.md;  python oracle.py --stop  when done

What is fair: identical rule text and examples (`examples.json` is transcribed from `loan.velme`); same model, 8192 max
output tokens, no retries beyond 3; each Velme trial starts from a fresh project and home, each B2 trial from an empty
directory; numbers are exact Decimal for B2 too (inputs arrive as Decimal, floats fail); neither arm sees the DMN, the
oracle or the held-out inputs. Installments are compared to 8 decimals (DMN 34 digits, Velme 28). `build_ok` in
`meta.json` means: Velme build succeeded / B2's final pytest passed. Velme's token log omits cache tokens: pass
`--no-prompt-cache` to run_velme.py for a full-price count (B2 uses no caching). Offline tests: `python -m pytest -q tests`.
