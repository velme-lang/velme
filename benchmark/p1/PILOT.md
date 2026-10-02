# P1 pilot: DMN 0087 on Sonnet, Velme vs Python plus tests

Run 2026-10-02/03 with `claude-sonnet-5-5`, Velme 0.1.0-alpha.1, harness at this directory's commit.
**Result: no-go for v0.1 as it stands.** No Velme build succeeded in 9 trials. The Python baseline (B2) passed all
200 held-out cases in all 4 of its trials. The full P1 waits for the language fixes below and then reruns this pilot
unchanged.

## Setup

- Program: the DMN TCK loan origination example, 10 decisions, restated in [requirements.md](0087/requirements.md).
  Both arms get that text and the same 33 examples ([loan.velme](0087/loan.velme), [examples.json](0087/examples.json)).
- Held-out: 200 inputs, 120 on or next to a rule boundary; expected outputs from Drools (Kogito JIT runner), which
  passes the TCK's own 8 cases and agrees with a hand-written reference on all 200.
- Velme ran with `prompt_cache = false`, so its token counts are full input, like the baseline's.

## Results

| Run | Trials | Held-out pass | Calls | Tokens in / out | $ per trial | Wall s |
|---|---|---|---|---|---|---|
| Velme, first requirements | 3 | 0% (no build) | 10.7 | 87.5k / 10.4k | 0.42 | 74 |
| Velme, input domain stated | 3 | 0% (no build) | 9.0 | 72.6k / 8.8k | 0.35 | 61 |
| Velme, same, `reply_format = "compact"` | 3 | 0% (no build) | 17.3 | 66.3k / 14.9k | 0.42 | 115 |
| B2, first requirements | 3 | 100% | 1.0 | 5.0k / 6.2k | 0.11 | 34 |
| B2, input domain stated | 1 (run stopped) | 100% | 2.0 | 16.6k / 13.1k | 0.25 | 67 |

$ at $3 / $15 per million input / output tokens.

## Why Velme failed

1. **Velme can't say which inputs a goal accepts.** Synthesis runs each goal on inputs generated from the parameter
   types alone (compiler/22 §7). `RequiredMonthlyInstallment` was given `term: -1, rate: -1`; the correct formula
   errors there (`range(-1)`), so the build was rejected twice and stopped (VL0403). Stating the domain in the plan
   text ("rate above 0 and at most 1, term a whole number from 1 to 600, otherwise the fee alone") fixed that goal,
   but only in words the checker can't use.
2. **Text stands in for enums, so every closed set is open.** With the domain stated, each build stopped on one goal
   with VL0407 "needs more detail": `RequiredMonthlyInstallment` (what fee for product type `"Lina"`?) twice,
   `BureauCallType` (what call type for risk category `"Lina"`?) once. The model was right to ask. The other goals
   built at the first attempt.
3. **`reply_format = "compact"` fails on this model.** Almost every goal failed with VL0401 "the reply had no `body`"
   or "not a usable reply", taking 17 calls and finishing nothing. Likely a Velme bug or prompt mismatch; the synth
   log keeps no replies, so it can't be confirmed from this run.

## Cost, had it built

About 7–8k input tokens per goal, almost all of it the fixed prompt, so a 10-goal program sends about 10 times what
the baseline sends once. Prompt caching would cut most of the input, but Velme's output alone (8.8k against 6.2k) is
already past the 1.5× go threshold. Cost per goal is the second risk after expressiveness.

## What the full P1 needs first

- **Input domains** (P0 in `POST-MVP.md`): closed Text sets (enums) and parameter ranges, used by the checker to
  generate valid inputs only and by the runtime to reject invalid ones. DMN already has both (`allowedValues`,
  `inputValues`). A language decision: spec, D-n, architect review.
- **Fix compact replies,** and log the failing reply's shape (not its text) so failures like 3 can be diagnosed.
- **Cut cost per goal:** measure cached runs (the synth log needs cache-read and cache-write token counts), and look at
  the fixed prompt size and the IR reply size.

Raw runs stay out of git (`runs/`); every trial kept its build output, synth log and lock for this report.
