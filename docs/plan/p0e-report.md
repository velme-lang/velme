# P0e report: reply notation against IR JSON

**Result: pass** on all three bars (`post-mvp-plan.md` §P0, P0e). Run 2026-10-15 on branch `p0-synth` after 5118537.
The live confirmation run passed on 2026-10-15 (§Live run), at a higher output cost than projected.

| Bar | IR JSON | Notation | Pass if | |
|---|---|---|---|---|
| First-attempt acceptance | 28 of 29 sent (1 not reached) | 30 of 30 | notation ≥ IR JSON − 1 goal | pass |
| Output per goal | 1,458 B (~442 tokens) | 320 B (~97 tokens) | notation ≤ 40% of IR JSON | pass: 21.9% |
| Projected $ per 10-goal build, cache on | $0.118 | $0.039 | ≤ $0.11 | pass |

## Method

- **Program:** the loan example, `crates/velme-synth/tests/notation/loan/loan.velme` (10 goals; DecideLoan reads the other
  nine as locals, so it is requested after them).
- **Path:** `velme build --provider external` against `tools/agent-bridge`, one bridge and one project per trial, default
  `[synthesis]` options except `reply_format`. Every reply went through the same parser, validator and examples as a
  provider's (INV-1); the scores come from `.velme/synth-log.jsonl` (`velme-synth-log/2`).
- **Provider stand-in:** one Sonnet subagent per request, at low effort, given only that request as rendered by
  `cargo xtask render-prompt --provider anthropic` (system, tools with input schemas, user messages) and told to write
  the `write_goal` input it would send. 3 trials per format, 60 requests.
- **Tokens and cost are estimates:** the bridge reports no tokens, so token counts are bytes / 3.3 (the spec's estimate).
  Prices assumed at Sonnet-class rates: $3/M input, $15/M output, cache write 1.25×, cache read 0.1×. Cost per build is
  one cache write of the shared prefix and nine cache reads, plus each goal's own part and its output. A prefix under
  1,024 tokens would not be cached; both prefixes are above it.

## Per goal

Mean first-attempt reply bytes over the trials in which the goal was answered.

| Goal | IR JSON | Notation | Ratio |
|---|---|---|---|
| Affordability | 1,291 | 261 | 20% |
| ApplicationRiskScore | 2,896 | 591 | 20% |
| BureauCallType | 695 | 157 | 23% |
| DecideLoan | 650 | 351 | 54% |
| Eligibility | 648 | 121 | 19% |
| PostBureauRiskCategory | 2,646 | 630 | 24% |
| PreBureauRiskCategory | 1,504 | 334 | 22% |
| RequiredMonthlyInstallment | 2,894 | 440 | 15% |
| Routing | 710 | 159 | 22% |
| Strategy | 650 | 157 | 24% |

DecideLoan is mostly record construction from named locals, where IR JSON is already close to the text; tables and
arithmetic shrink most.

## Requests

| | IR JSON | Notation |
|---|---|---|
| Shared prefix (system, tools, first user block; identical for all 10 goals) | ~5,480 tokens | ~1,350 tokens |
| Per-goal part, all 10 goals | ~5,380 tokens | ~5,370 tokens |
| Output, all 10 goals | ~4,420 tokens | ~970 tokens |
| Cost per build, cache on / off | $0.118 / $0.247 | $0.039 / $0.071 |

The notation's saving on input is the prefix: `write_goal` takes `{"body": string}` instead of the IR schema. Its prefix
is 1.3× the 1,024-token cache minimum, so a few hundred bytes cut from `notation.txt` would stop it being cached.

## Failures and caveats

- **IR JSON, trial 3, ApplicationRiskScore:** the reply was invalid JSON (closing braces after the end of a 2.4 KB
  nested tree); the bridge holds an answer that does not parse as pending, so the goal never reached Velme and DecideLoan
  was not requested (`VL0409`). Through a real provider's tool call, the API would have rejected or repaired it
  differently; it is scored as a first-attempt failure. No other request in either format failed, so no retry ran.
- **The stand-in is not the API.** Five IR JSON stand-ins wrote a short Python script to generate their nested JSON, which
  a model answering an API request cannot do. That helps IR JSON's acceptance, not the notation's, so the pass stands, but
  IR JSON's acceptance here is likely an upper bound.
- No stand-in read anything but its own request (checked from the agent transcripts).
- The loan goals are easy for the model: 59 of 60 accepted on the first attempt leaves acceptance with little room to
  separate the formats. The live run and the P1 benchmark are the real test of acceptance.
- The run found one P0d gap, fixed with this report: `velme.toml` rejected `reply_format = "notation"`
  (`crates/velme-runtime/src/config.rs`).

## Live run

`velme build --provider anthropic` on `claude-sonnet-5-5`, `reply_format = "notation"`, at 2c0c873, one fresh project per
run. Tokens are the API's, cost at the rates above.

| Run | Built | First attempt ok | Requests | Output tokens | Cost |
|---|---|---|---|---|---|
| 1 | 7 of 10 | 6 of 9 sent | 10 | 3,704 | $0.089 |
| 1, rebuilt | 7 of 10 | 0 of 2 | 2 | 781 | ~$0.01 |
| 2, after the plan fix | 10 of 10 | 8 of 10 | 12 | 4,702 | $0.111 |

- **Questions the stand-ins never asked.** In run 1 the model asked about RequiredMonthlyInstallment (the fee for a
  product type other than the two named) and BureauCallType (a category outside the five named), and DecideLoan was not
  reached (`VL0409`). Both were real gaps in the plans; the hand-written replies already took the last case as the
  default, so the plans now say so. The stand-ins always answered, so P0e measured no questions.
- **Output is about 5× the estimate.** Run 2's replies total ~3.1 KB (~940 tokens, close to P0e's 320 B per goal); the
  rest of the 4,702 output tokens is text the model writes beside the tool call, which the synth log does not keep.
  It is most of the cost: $0.111 against the projected $0.039. It applies to any reply format, so the format choice
  stands; trimming it is a follow-up (prompt or `tool_choice`).
- **Replies without a tool call.** 3 of 22 live requests ended with `end_turn` and no `write_goal` call
  (`malformed-not-one-tool`); each retry succeeded. `tool_choice` is `auto` (D-150).
- Caching works as projected: one 2,014-token prefix write per build, then reads.
