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
