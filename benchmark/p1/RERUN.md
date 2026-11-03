# P1 pilot rerun: DMN 0087 on Sonnet, Velme 0.2 with input domains

Run 2026-11-03 with `claude-sonnet-5-5` and Velme 0.2.0-alpha.1, using the harness at this directory's commit.
The run is the [pilot](PILOT.md) unchanged except for the source: `loan.velme` now uses `velme/0.2` input domains.
These are closed Text sets for every enum, and ranges for age, rate, term and credit score. One prose workaround is
gone, the installment's "rate not above 0 … fee alone" sentence, because the `Rate` and `Term` domains now state it.
Requirements, examples and held-out inputs are unchanged. Velme ran with `prompt_cache = false` and the default
`notation` reply format.

**Result: go for the full P1.** All 3 Velme builds succeeded. Every trial passed all 200 held-out cases, and every goal
was accepted at its first attempt. The cost per trial was about level with B2's first run and about half its second.

## Results

$ at $3 / $15 per million input / output tokens. The B2 rows are the pilot's, reused because the B2 arm is unchanged.

| Run | Trials | Held-out pass | Calls | Tokens in / out | $ per trial | Wall s |
|---|---|---|---|---|---|---|
| Velme 0.2, domains, notation | 3 | 100% | 10.0 | 29.1k / 2.7k | 0.13 | 32 |
| B2, first requirements | 3 | 100% | 1.0 | 5.0k / 6.2k | 0.11 | 34 |
| B2, input domain stated | 1 (run stopped) | 100% | 2.0 | 16.6k / 13.1k | 0.25 | 67 |

The pass rate was 100% on both the 120 boundary-tagged cases and the random-tagged ones, with no field failures,
errors or floats.

## Failure causes

There were none. The synth log (`velme-synth-log/2`) has 30 entries, one per goal per trial. Each is `attempt 0`,
`outcome "ok"` and `cause null`, with a `thinking` + `tool_use` reply carrying `body`. Each fix from the pilot cleared
its failure:

1. **Domains:** the VL0403 rejection on `RequiredMonthlyInstallment` (`term: -1`) did not recur, because generated
   inputs now stay inside `Rate` and `Term`.
2. **Closed sets:** the VL0407 "needs more detail" for product type or risk category `"Lina"` did not recur either.
3. **Notation:** the notation reply format replaced compact, and none of its replies failed.

## Cost

- **Velme:** about 2.9k input and 270 output tokens per goal, against about 8k input in the pilot.
- **B2:** compared with its first run, Velme sends about 6× the input but less than half the output. Output is the
  costlier side, so the totals land within $0.02.

These are uncached numbers. Caching would bill the fixed prompt prefix and reply schema at 0.1×, so a cached build
would cost less, but this run did not measure that.

## Caveats

- One program, one model, 3 trials. B2's numbers are reused, not rerun.
- `Eligibility(age: Number)` keeps its prose "under 18" rule. Its example `Eligibility("LOW", true, 17)` lies outside
  `Age`, and the examples could not change. The `Applicant.age` field does use `Age`.
- Velme's input-domain checks have never seen an input outside a domain, since all 200 held-out inputs fall inside.

Raw runs stay out of git (`runs/velme-rerun`). Each trial kept its build output, synth log and lock.
