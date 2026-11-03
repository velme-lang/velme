# Velme — post-MVP plan

**Goal:** make Velme able to state a rule's valid inputs and cheap enough per goal to beat a "model writes Python
plus tests" baseline on real business logic, then prove it on the P1 benchmark.
**Read when:** starting or finishing a phase. Read **only the phase you are working on**. Phase status is public
through git, as in the MVP plan (R-REL-12, D-66).

**Why:** real rules have input limits (a term of 1 to 600 months, a product type from a fixed list) that v0.1 can't
state, so synthesis checks correct code on values the rule never accepts. And IR JSON replies are several times the
size of the same logic in a familiar language, with a large reply schema in every request.

---

### P0 — Synthesis replies (part 1 of P0)

**Read:** `compiler/22` §3.2, §4, §4.1, §5, §10; `compiler/21` §1, §3, §6, §7; `SPEC` INV-1; D-44, D-93, D-97,
D-103, D-104.
**Slices:**
- **S0** spec commit: D-152 (reply notation, experimental), INV-1 reworded, D-44 revised to the measured saving,
  R-SYNTH-36 compact fixes, R-SYNTH-23 log `velme-synth-log/2`, rejected alternatives (replies in Velme syntax,
  several goals per request).
- **P0a** compact replies fixed: the contract names `body` by its alias, earlier replies are shown as received,
  aliases are disjoint from canonical names, template wording is provider-neutral, a `Malformed` reply keeps its
  reason (a closed id; all `Malformed` attempts are one cause); tests feed a compact reply under the alias, a reply
  under the canonical `body` key (a failed attempt with a clear `VL0401` cause, D-153), and a compact retry turn.
- **P0b** synth log v2: `cause`, `stop_reason`, the reply's shape (content block kinds and top-level keys, IR keys
  only) and cache-read and cache-write tokens; never plan or reply text (R-SYNTH-23).
- **P0c** agent bridge, dev tooling only: `cargo xtask render-prompt` renders the exact provider request for a goal
  with no network; `tools/agent-bridge` is an `external` backend that answers from an answers folder and queues the
  rest as `pending` (R-SYNTH-41), so a coding agent can stand in for the provider; a runbook.
- **P0d** reply notation, experimental (`reply_format = "notation"`): a Python-shaped subset defined by its lowering
  table to `compiler/21` §3 nodes, parsed by a total, fuzzed module in `velme-synth` before the unchanged validator,
  with spans for retry messages, empty-list types from context, and the grammar version in `prompt_version`. Tested
  offline on the 10 goals of the loan example written by hand.
- **P0e** experiment, through the agent bridge: 3 builds with IR JSON against 3 with the notation on the loan example,
  scored per goal; report. Pass: first-attempt acceptance no worse than IR JSON minus one goal, notation output at
  most 40% of IR JSON's, projected cost per 10-goal build at most $0.11 with caching.

**Order:** S0 → P0a → P0b → P0c → P0d → P0e. P0b's log format gets an architect review before commit (public
contract); P0d's grammar and lowering get one before commit. Every review of the IR or a reply format judges token cost
(input per request, output per goal) as well as correctness.
**Exit:** `cargo xtask verify` green; the P0e report committed with a pass or fail against its bar.
**User verifies:** the P0e report; then one live confirmation run (≤ $1) before the notation becomes the default.

### P0 — Input domains (part 2 of P0)

Planned after an architect review of two owner decisions (domains in IR 0.2, conditions beyond simple bounds):
`velme/0.2` only, domain types in syntax, semantics, IR 0.2 and the validator, `VL0504` output checks, the input
generator, prompt and request field; `pow`; the notation as default if P0e passes; then the pilot reruns.

### P1 — Real-goals benchmark (go/no-go)

Answers: does Velme write real business logic more correctly than the obvious alternative, at a similar LLM cost?
A live, manual run, never part of `cargo xtask verify` (D-13); the harness lives in `benchmark/p1/`, outside the CLI,
and the results are committed as a report.

**Pilot (done).** DMN 0087 on Sonnet against B2 below. On v0.1 it was a no-go: no Velme build succeeded
([PILOT.md](../../benchmark/p1/PILOT.md)), which led to both parts of P0. Rerun unchanged on `velme/0.2` with input
domains, it was a go: 200/200 held-out cases in 3 of 3 trials at $0.13 per trial, against B2's $0.11 and $0.25
([RERUN.md](../../benchmark/p1/RERUN.md)).

**Corpus:** 8 programs. External sources make it hard to say the corpus was cherry-picked.
- *DMN TCK* (compliance-level-3), the OMG decision-model conformance suite: `0087-chapter-11-example` (loan
  origination, the pilot), `0020-vacation-days` (overlapping age and service thresholds), `0014-loan-comparison`
  (ranking lender products by monthly payment: lists, `sort_by`, `pow`).
- *Our own, in the domains business-logic authors ask about:* order pricing (tiers, stacked discounts, coupon caps),
  VAT and shipping by region and weight band, insurance quote (base rate, risk loadings, caps), invoice validation
  (line totals, tax, rounding to the cent, reasons for rejection), refund policy (days since purchase, item state).

**Oracle and held-out tests:** expected outputs come from a reference neither arm sees: for DMN, the model run in
Drools, checked against the TCK's own cases; for our programs, a hand-written Python oracle, written before either arm
runs. 200 generated inputs per program, weighted to land on and next to every boundary in the rules. Values compare as
exact decimals; a float in the baseline counts as a failure and is reported in its own column.

**Arms,** all from the same plain-English rules and visible examples, same model, same retry budget (3):
- **Velme:** `velme build`, then `velme run` over the held-out inputs.
- **B1, one-shot Python:** one request returns one function. The floor.
- **B2, Python plus tests in a loop:** the model writes the function and pytest tests from the examples, runs them,
  and fixes up to 3 times. The bar: beating only B1 proves nothing.

Models: Haiku and Sonnet decide; one local model is reported only.

**Metrics:** held-out pass rate (boundary and random cases apart), $ and tokens per program, provider calls, wall
time, goals that never built.
**Go:** on Haiku and Sonnet, Velme's held-out pass rate is at least B2's and its cost at most 1.5× B2's; or, with
pass rates equal, it fails fewer boundary cases. **No-go:** stop and rethink before any launch work.
**Slices:**
- **P1a** harness for a corpus: one folder per program under `benchmark/p1/`, every script takes `--program`; the B1
  arm (B2's first prompt without the tests instruction, one call, no feedback); scoring pooled per model and per
  program, boundary and random cases apart, floats apart; prompt caching on for every arm, with the uncached cost
  reported beside it; offline tests.
- **P1b** DMN `0020` and `0014`: requirements, signatures, examples, `.velme` source, Drools oracle checked on the
  TCK's cases, 200 held-out inputs each.
- **P1c** order pricing, VAT and shipping, insurance quote; **P1d** invoice validation, refund policy: requirements and
  examples, then a Python oracle written from the requirements alone by a different model family than the ones under
  test. Each requirements file is read by the owner before its held-out inputs are fixed.
- **P1e** live runs: 5 trials per program, arm and model (≤ $30), then `benchmark/p1/REPORT.md`.

Held-out inputs stay inside each program's stated input ranges. The go bar pools each model over all programs and
trials; a Velme build that fails counts as 0/200 for that trial. A program Velme can't fully express stays in, and the
gap goes in the report's list of missing features.
**Order:** P1a → P1b → P1c → P1d → P1e.
**User verifies:** the P1 report.
