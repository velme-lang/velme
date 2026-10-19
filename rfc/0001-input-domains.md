# RFC 0001: Input domains

- **Status:** draft (owner decisions D-151 and D-158–164 settled; implementation pending D1–D5).
  This RFC must reach **accepted** before merge (R-REL-11); D0 does not grant that approval.
- **Spec ids touched:** R-SYN-15, R-SYN-23–24; R-TYP-28–33; R-CHK-13–15; R-IR-24–26;
  R-SYNTH-60–66; R-RUN-26–28; R-ART-26–27; R-CLI-29; R-ERR-04; R-REL-02, R-REL-14;
  D-151, D-158–164; Q-10, Q-17, Q-33–34.
- **Invalidates locks:** yes — one coordinated `0.2.0-alpha.1` release.

## Summary

Named domains describe accepted base values: closed Text sets and Number, Text or record types with a Boolean
`where` condition. They preserve base JSON representation, retain identity in IR `0.2`, and constrain generated
inputs and checked inputs/outputs. The release accepts only `velme/0.2`; a headerless file means `0.2`.

## Motivation

A parameter typed `Text` admits misspellings of a risk category; unconstrained Number generation tries negative
ages and fractional loan terms. Authors need to state their accepted values once and have syntax checking,
generation, synthesis and execution agree. Record conditions also express relations such as discount ≤ price.

## Design

```velme
language: velme/0.2
type Risk = Text one of "DECLINE", "HIGH", "VERY LOW"
type Age = Number where value >= 18 and value <= 120 and value is whole
type Label = Text where value.length > 0 and value.length <= 40
type Offer where value.discount <= value.price:
  price: Number
  discount: Number
```

`where` is a keyword; `value`, `one`, `of` and `whole` are contextual. Domain declarations are named, never inline
parameter/field annotations. The condition uses the check expression language with `value` bound to its base
value. A condition's value error means non-membership; budget/internal failures keep their own diagnostics.
R-TYP-29 and R-CHK-14 define invalid declarations (`VL0209`), including empty/duplicate sets, wrong-base
constraints, repeated interval bounds and empty representable intervals. Sets allow at most 1 000 members and
64 KiB total UTF-8 across the decoded members. Domains have no static exhaustiveness check.

A domain widens to its base, including through List and optional wrappers, as an explicit exception to List
invariance. Direct different-domain expressions are rejected where a domain is expected; identity-preserving
nodes retain domains, fixed-signature operations yield bases, and distinct-domain/base joins widen (D-163).
Static rejection is best effort; deep output membership is the sound guarantee. Equality/operators first widen
both sides, while the non-member-literal comparison rule still applies. Source literals are checked against closed sets/simple
constraints through expected types (`VL0204`); `velme-check` then evaluates full source-literal membership from
HIR, including residual predicates, before a literal is admitted as a domain call argument or example. Source
literals are fully constant; domain-typed paths come from checked inputs/outputs, so calls need no extra cast or
membership boundary. Comparing a closed set with a non-member literal is also rejected.
IR validation applies the same simple literal checks (`VL0402`) and permits implicit base-valued construction at
outputs, record fields and list items, recursively through List/Optional and expected branches via `compat`
(D-163, R-IR-25). No cast expression is needed. Bodies never contain Domain references: body annotation schema
definitions exclude them (`VL0401`); list inference writes bases while the outer expected-type pass retains domains.

IR `0.2` has a Domain type and a compiler-owned `domains` table of bases, set members and simple constraints.
The compiler completes it and validates it against the source signature; the model returns only a body.
Residual conditions remain in validated HIR and are evaluated by `velme-check`. WASM represents domains as their
bases. The membership walk lives in `velme-check`, called only by `velme-runtime` around backend invocations,
not via a WASM host import. Fuel left after input validation becomes the body's `velme_fuel_left` (or interpreter
equivalent); output validation uses the backend's reported remainder. Verification/examples use the same input
walk and fuel; invalid generated/example input membership is `VL0607`. Closed sets use a non-allocating lookup;
predicate allocations count identically on both backends. JSON and canonical value hashing retain the
base representation. Deep JSON input membership failures use `VL0902`, exit 64; every goal's output is checked
before its own checks with `VL0504`, including nested record fields, list items and present optionals. Validation
is metered by R-RUN-27 and preserves backend fuel/diagnostic parity.

Generation extracts top-level conjunctive Number bounds, including reversed literal comparisons, `is whole` and Text scalar-length bounds for
exact generation, including both edges; only negative or fractional literal Text length bounds are `VL0209`.
Computed Text length bounds stay residual filters. Random Text lengths draw from `lo` through `min(hi, lo + 16)`;
boundary sets still cover `hi`.
Exclusive non-whole Number edges use ±0.01, or the midpoint for a narrower interval. Random draws use the interval directly;
finite non-whole intervals mix whole/2-decimal grids. Whole and grid draws cap the point count at 2⁶⁴,
counted from the lower admissible index, preserving the 128-bit-product `below` definition. If exactly one grid is empty, use the other without consuming
`below(2)`; if both are empty, use the R-TYP-04-rounded midpoint without grid-selection/index draws, counting
duplicate valid midpoints normally in the probe (R-SYNTH-62, AC-SYNTH-70, D-164). Other conditions filter complete values. Finite sets are filtered in
order; a random value receives an initial draw and K = 16 retries, redrawing a whole record for a relational
condition. Empty sets propagate through wrappers (`[nothing]` / `[[]]`), record/parameter diagonals, then skip
only resulting empty boundary/pairwise stages. Examples count descriptively toward coverage without changing
value sets/order beyond the existing tuple de-duplication. Every closed-set member is covered up to 16 members
within the existing total/slot cap. R-SYNTH-62–66 specify projection, ordering, draw consumption and V4–V6 fixtures.

Before synthesis, `velme check` probes each domain with 64 raw draws seeded by that declaration's own hash,
independently of goals/models; fewer than 8 valid draws is `VL0210`. Duplicate successes count. This prevents an
impractical filter from consuming synthesis attempts. The prompt shows full declarations, typically 20–25 tokens
each, rather than erasing their conditions. External request `0.2`, `/v2` endpoints and `input_version` are specified
by R-SYNTH-61. Use declaration source text with comments stripped; render record domains once in declaration
form, and charge uncached declaration tokens on every retry. All reply formats receive compiler-owned tables
and the same body schema, which excludes Domain annotations.

Signatures hash full normalized declarations, including residual conditions and nested field contracts.
Types named by record constructors in conditions are transitive dependencies too. The canonical signature
document is `{domains, goal, inputs, output, types}`; `domains` holds full normalized declaration nodes, while
parameter types reference e.g. `{"t":"Domain","name":"Age"}` (R-ART-26). Tightening a child's domain changes its callers' keys. This closes Q-17's alias/key points. Q-10 now concerns
relations across separate parameters; Q-33 retains further Text rules and price decimal-place constraints.
Q-34 defers parenthesized multi-line `one of (…)` past 0.2; record domains use `type Offer where <cond>:`
with indented fields, and `type Adult = Person where …` is an unsupported base (`VL0209`).

The CLI, language and IR migrate together at `0.2.0-alpha.1`: only `velme/0.2` and IR `0.2` are accepted;
`velme/0.1` reuses `VL0106` with help to change the header and rebuild. D1 re-blesses repository fixtures without
provider calls, records the break and `invalidates locks: yes` in CHANGELOG, and creates the versioned
`language/spec/0.2/` mirror. D0 changes the specification only. D1 handles release/version migration, D2 syntax,
typing and keys, D3 IR/prompt/request integration, D4 membership/fuel/parity, and D5 generation/probes/goldens.
Until D5, domain programs use base-type generation; D2–D4 do not claim end-to-end domain builds.
`pow` ships in the same release, specified in D6.

## Alternatives and drawbacks

- **Erase domains to base types:** rejected. Model-written IR could bypass the contract, and artifact types would
  lose the declarations needed for validation and compatibility.
- **An explicit cast node:** rejected. It costs reply tokens and asks models to write domain references; implicit
  expected-type construction plus deep output checking supplies the enforcement point.
- **Bare-name enums:** rejected. Existing Text values, including spaces, must round-trip without JSON wrappers;
  tagged enums remain available for a later design.
- **Bounds only:** rejected by the owner. Arbitrary check conditions and record relations are useful now;
  deterministic value filtering and a scarcity probe bound their cost. Patterns remain filter-only.
- **Keep both language/IR versions:** rejected. Only the quiet alpha exists; one pre-launch rebuild avoids two
  compatibility paths. Full declarations add uncached prompt tokens and validation consumes invocation fuel.

## Effect on invariants

INV-1/INV-2 remain intact: conditions use validated check lowering and the reference evaluator, never a model
judgment, and model-written bodies still pass full IR validation. INV-3 is preserved by declaration-local probe
seeds, fixed retry limits and shared interpreter/WASM membership semantics. INV-4 gains no host capability.
INV-5 retains bounded traversal, fuel and existing value limits. INV-8 is preserved by full-declaration signature
hashing and the coordinated version break; base JSON values do not gain identity tags.

INV-9 remains intact: `velme-synth` owns domain draws and the scarcity probe, calls `velme-check` for predicates,
and runs during `velme check` without a provider. Only `velme-runtime` invokes the membership walk around
backend calls; `velme-check` never depends on synth, and neither backend depends on check. No new crate edge or
WASM import is added. No invariant is relaxed.
