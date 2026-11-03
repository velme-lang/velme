# P1 report: real-goals benchmark, Velme vs Python

Run 2026-11-03 with Velme 0.2.0-alpha.1 (`velme/0.2`, default `notation` reply format, prompt caching on) and the
harness at this directory's commit. 8 programs × 3 arms × 5 trials per model, 200 held-out cases per trial. The local model the plan listed as
reported only was dropped (owner, 2026-11-03). Plan:
[post-mvp-plan.md §P1](../../docs/plan/post-mvp-plan.md).

P1 asks whether Velme's guarantees cost anything: does a program built through Velme (checks and examples run by the
runtime, never an LLM; only validated IR, in a sandbox; the same build again offline) give up correctness or LLM
spend against a model writing Python plus tests?

**Result: go on both deciding models. Velme gave up no correctness and cost less.** Every arm passed every held-out
case in every trial, on Haiku and on Sonnet; Velme cost 0.79× B2 on Haiku and 0.75× on Sonnet, inside the 1.5× bar.

## Pooled per model

$ as billed, from Anthropic list prices: Haiku 5.5 $0.10 / $0.50 and Sonnet 5.5 $2 / $10 per million input / output
tokens, cache reads 0.1× input, cache writes 1.25×. "Uncached" bills every input token at the plain input price.
Per trial, averaged over 40 trials (8 programs × 5).

| Model / arm | Pass | Boundary | Random | Floats | Failed builds | $ / trial | Uncached $ | Calls | Wall s |
|---|---|---|---|---|---|---|---|---|---|
| Haiku / B1 | 100% | 100% | 100% | 0 | 0 | 0.0011 | 0.0013 | 1.00 | 8.0 |
| Haiku / B2 | 100% | 100% | 100% | 0 | 0 | 0.0028 | 0.0031 | 1.15 | 18.5 |
| Haiku / Velme | 100% | 100% | 100% | 0 | 0 | 0.0022 | 0.0027 | 2.90 | 15.2 |
| Sonnet / B1 | 100% | 100% | 100% | 0 | 0 | 0.0152 | 0.0192 | 1.00 | 8.2 |
| Sonnet / B2 | 100% | 100% | 100% | 0 | 0 | 0.0330 | 0.0372 | 1.00 | 18.0 |
| Sonnet / Velme | 100% | 100% | 100% | 0 | 0 | 0.0248 | 0.0341 | 2.77 | 14.0 |

Go bar (per model, pooled): Velme's pass rate ≥ B2's and its cost ≤ 1.5× B2's.
- **Haiku: GO.** 100% vs 100%, $0.0022 vs $0.0028.
- **Sonnet: GO.** 100% vs 100%, $0.0248 vs $0.0330.

Total spend for the run: about $3.25 (Haiku $0.24, Sonnet $2.92, smoke run $0.07).

## Per program ($ / trial; every cell 100% held-out pass)

| Program | Goals | Haiku B1 | Haiku B2 | Haiku Velme | Sonnet B1 | Sonnet B2 | Sonnet Velme |
|---|---|---|---|---|---|---|---|
| 0087 loan origination (DMN) | 10 | 0.0022 | 0.0038 | 0.0041 | 0.0319 | 0.0572 | 0.0517 |
| 0020 vacation days (DMN) | 4 | 0.0005 | 0.0012 | 0.0008 | 0.0088 | 0.0226 | 0.0136 |
| 0014 loan comparison (DMN) | 4 | 0.0014 | 0.0097 | 0.0032 | 0.0226 | 0.0547 | 0.0388 |
| order pricing | 1 | 0.0010 | 0.0016 | 0.0012 | 0.0116 | 0.0241 | 0.0156 |
| VAT + shipping | 1 | 0.0010 | 0.0016 | 0.0025 | 0.0116 | 0.0285 | 0.0209 |
| insurance quote | 1 | 0.0012 | 0.0019 | 0.0015 | 0.0133 | 0.0253 | 0.0180 |
| invoice validation | 1 | 0.0009 | 0.0015 | 0.0022 | 0.0088 | 0.0260 | 0.0211 |
| refund policy | 1 | 0.0009 | 0.0015 | 0.0018 | 0.0128 | 0.0254 | 0.0184 |

Velme costs more than B2 in 4 of 16 cells, all on Haiku: 0087, VAT + shipping, invoice validation, refund policy.
On Haiku, Velme's output tokens for one-goal programs run 0.7–1.6× B2's, which writes a solution plus its tests; on
Sonnet, Velme's replies are shorter than B2's on every program.

## Retries and fixes

- **Velme:** 7 failed attempts over 80 builds, all retried and accepted within the budget: Haiku 6 (0014 trials 1, 2,
  5; 0087 trials 4, 5 ×2), Sonnet 1 (0087 trial 2). The harness keeps only the synth log's totals per trial, not each
  attempt's cause, so the causes are not reported (a harness gap for the next run).
- **B2:** Haiku used its fix loop 6 times over 40 trials, all on 0014 (its first pytest run failed); Sonnet never did.
- **B1:** one call by design. It passed every case anyway.

## What the run does and doesn't show

- **Parity on correctness.** All three arms hit 100% on all 8 programs, both models, boundary and random cases
  alike, so the corpus didn't separate them; B1, one call with no tests, was the cheapest arm on every program.
- **What it doesn't measure** is what Velme is for: who decides a result is right, what the model's output is allowed
  to do, what a reviewer reads, and whether a rebuild gives the same program. Those come from the design (INV-1 to
  INV-7), not from pass rates; P1 shows they come at no cost in correctness and below B2's cost.
- **Multi-goal programs cost Velme calls.** One provider call per goal: 0087 averaged 10.2–10.6 calls per build,
  against B2's 1. Caching keeps that cheap (0087 Sonnet Velme $0.0517 vs B2 $0.0572), but wall time is the highest
  of any cell (31–35 s, rounded).
- **Earlier cost figures were overstated.** The pilot and rerun reports priced `claude-sonnet-5-5` at $3 / $15; the
  list price is $2 / $10. Their Velme-to-B2 ratios hold, since every arm used the same prices; their absolute $
  figures are 1.5× too high.

## Missing features (programs Velme could express only with workarounds)

All 8 programs built; none was dropped. The fit gaps found while writing them:
- **Examples can't state approximate values** (0014): its TCK expected payments are doubles, so the examples give
  exact decimals instead.
- **0014's metrics live in one goal** rather than one per metric, since a goal returns one record list.
- **`round` has one tie mode and no places argument** (order pricing, VAT, insurance, invoice): cents are rounded by
  scaling, rounding and scaling back.
- **A record type can't share a goal's name** (VL0203), so result records take a different name from their goal.
- **No conditional list append** (invoice validation): rejection reasons are built with a filter over an if/else
  that yields `""`.

## Reproduce

See [README.md](README.md) for the run order. Results are written to `runs/p1e/` (`python score.py runs/p1e`); like the pilot runs they are not committed (79 MB).
