# 13 — The `check` Assertion Language

**Status:** v0.1 · **Area:** CHK
**Read when:** changing check type rules, check evaluation, check failure reports, or how checks are lowered for the runtime.
**Depends on:** [SPEC](../SPEC.md), [10-syntax-grammar](10-syntax-grammar.md) §4, [11-types](11-types.md), [14-builtins](14-builtins.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-6, D-20)

## 1. Purpose & boundaries

`check` is a small, deterministic assertion language. The runtime — never an LLM — decides whether a result passes
(INV-2). The grammar is the `expr` production of [10-syntax-grammar](10-syntax-grammar.md) §4; operator types are in
[11-types](11-types.md) §8; helper signatures in [14-builtins](14-builtins.md). A check is a property of the result,
not a proof: it is evaluated on concrete inputs.

```text
check:
    - result > 0 and result < 10
    - result.player.name != ""
    - every ball in result has ball.bounce >= 5
    - if players is not empty then result is not empty
```

**R-CHK-01** Each `- expr` item must have type `Boolean`; otherwise `VL0204` ("a check must be true or false").

## 2. Scope

**R-CHK-02** Names visible in a check:

| Name | Type | Available |
|---|---|---|
| each parameter | declared type | always |
| each `call` binding | callee output type | composite and wired goals |
| `result` | goal output type | always |
| quantifier variable | element type | inside its `has` body |

A quantifier variable that repeats a visible name is `VL0306 DuplicateBinding`. Unknown names are `VL0202 UnknownName`.

**R-CHK-03** Checks call only built-ins marked "checks" in [14-builtins](14-builtins.md). Calling a goal from a check is
`VL0303 InvalidCall`: a check observes one execution, it never starts another.

## 3. Constructs

| Construct | Meaning | Notes |
|---|---|---|
| `a == b`, `!=`, `<`, `<=`, `>`, `>=` | comparison | R-TYP-20 table |
| `a + b`, `-`, `*`, `/`, unary `-` | `Number` arithmetic | D-6; errors per §5 |
| `a and b`, `a or b`, `not a` | Boolean logic | short-circuit, left to right |
| `if a then b` | implication, ≡ `not a or b` | D-6; `b` not evaluated when `a` is false |
| `x is empty`, `x is not empty` | emptiness of `T?`, `List`, `Text` | R-TYP-20 |
| `x is whole`, `x is not whole` | Number has no fractional part, or its negation | R-TYP-07; domains use the base; nullable operands need narrowing |
| `x.field` | field access | `VL0205` if unknown; `VL0207` on un-narrowed `T?` |
| `xs.field` | projection → `List<F>` | R-TYP-14 |
| `xs.length`, `t.length` | length | |
| `maximum(xs)`, `minimum(xs)`, `sum(xs)`, `contains(xs, v)` | collection helpers | `maximum`/`minimum` return `Number?` |
| `every x in xs has P` | all elements satisfy `P` (true for `[]`) | |
| `some x in xs has P` | at least one element satisfies `P` (false for `[]`) | |
| literals, record literals, list literals | values | R-TYP-17 |

**R-CHK-04** Narrowing (R-TYP-22) applies inside checks. Because `maximum` returns `Number?`, "biggest" properties are
written with a quantifier, which reads naturally and needs no narrowing:

```text
- if result is not empty then every p in players has result.jump_height >= p.jump_height
```

## 4. Evaluation

**R-CHK-05** Checks are evaluated after the goal produces its output, on every run (D-20) and on every verification
input. Every item is evaluated, in source order; each item is independent (narrowing does not cross items).

**R-CHK-06** Evaluation is deterministic: `and`/`or`/`if` short-circuit left to right; quantifiers visit elements in list
order and stop at the first element that decides the answer (first counterexample for `every`, first witness for
`some`).

**R-CHK-07** Errors raised while evaluating a check item — `VL0602 ArithmeticError` (e.g. division by zero), or any
other value error — make **that item fail**. The goal fails with `VL0501 CheckFailed`, and the report attaches the
underlying code as the cause ("could not evaluate `a / b`: VL0602 division by zero"). Rationale: a check that cannot be
shown true is not satisfied, and learners see one failure kind for a broken contract.

**R-CHK-08** Check evaluation is charged to the goal invocation's budget. Exhausting a budget during checks is the
budget failure (`VL0601`, `VL0603`, `VL0604`, `VL0606`), not `VL0501` — the run was stopped, the check did not fail.

**R-CHK-09** If several items fail, all are reported in source order; the goal's failure (and its trace entry, D-9)
cites the first.

## 5. Failure report

**R-CHK-10** A `VL0501` report contains:

| Part | Example |
|---|---|
| goal and failing assertion, as written, with source span | `BuildPlayerSummary` · `result.rank == rank` |
| values of every path and helper call in the assertion | `result.rank = 3`, `rank = 4` |
| for `==` / `!=` / ordering: Expected / Received | Expected `4`, Received `3` |
| for `every` / `some`: the deciding element's index and value | `result[2] = Ball(bounce: 3)` fails `ball.bounce >= 5` |
| for short-circuited operands: marked "not evaluated" | |
| cause code, if R-CHK-07 applied | `VL0602` |
| the goal's inputs and binding values (via the trace) | [30-execution-vibevm](../runtime/30-execution-vibevm.md) |

Values render in the canonical JSON form ([11-types](11-types.md) §10), truncated for display as in
[30-execution-vibevm](../runtime/30-execution-vibevm.md) R-RUN-20, with the full value
available in `velme trace --json`.

## 6. Lowering

**R-CHK-11** The compiler lowers each check item to the IR expression subset ([21-ir](../compiler/21-ir.md)), whose
meaning the reference interpreter defines, so a check means what the goal's own IR would (INV-3). Checks and examples
always evaluate on the reference interpreter, whichever executor ran the goal body (D-80): a report depends only on the
lowered check and the invocation's values and spent budget, which INV-3 makes the same on every backend. Lowering is
deterministic and never involves an LLM:

| Surface | Lowered form |
|---|---|
| `every x in xs has P` | `all(xs, x -> P)` |
| `some x in xs has P` | `any(xs, x -> P)` |
| `if A then B` | `or(not A, B)` (short-circuit) |
| `xs.field` (projection) | `map(xs, e -> e.field)` |
| `x.length` | `length(x)` |
| `x is empty` / `is not empty` | `is_empty(x)` / `not is_empty(x)` |

Each lowered node keeps its source span so reports (§5) point at the learner's text. A lowered check or example passes
the validator's structure, name and type stages (21 §6 stages 2–4: no `call` node, no name bound twice) in its goal's
check scope — its inputs, call bindings and `result`, and every record type of the program — before anything evaluates
it, so the interpreter runs only validated IR (INV-1, D-84); a failure there is a compiler bug (`VL0607`). Stage 7's
limits bound what an LLM writes, not checked source, which can pass them (a 200-operator chain, five nested
quantifiers, a 1 001-item list); a lowered check's depth is bounded by its source line instead (`runtime/30` R-RUN-25).

**R-CHK-12** Checks are part of the goal's normalized source and therefore of its fingerprint (D-11); they are sent to
the LLM as the specification to satisfy ([22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md)), but their
truth is decided only here.

### 6.1 Domain conditions

**R-CHK-13** The expression after `where` uses this chapter's complete check expression language, with one
additional binding `value` of the unconstrained base type (the underlying record for a record domain). It must
return Boolean. It sees no goal parameters, call bindings or `result`; only `value`, its quantifier variables,
record constructors and check-callable built-ins. Record conditions may relate fields, e.g.
`value.discount <= value.price`. Type errors in the condition are `VL0209` (R-TYP-29). Outside a declaration,
`is whole` is also available in ordinary checks: `x is whole` lowers to `floor(x) == x`, and `is not whole` to
its negation, with Number operands required (`VL0206`, or `VL0207` before nullable narrowing).

**R-CHK-14** Split a condition by flattening only its top-level `and` tree, in source order. Extract only:

| Conjunct | Exact-generation constraint |
|---|---|
| `value > n`, `>= n`, `< n`, `<= n` on Number, or the reversed literal comparison | one lower / one upper bound, exclusive / inclusive |
| `value is whole` on Number | integer-valued generation |
| `value.length > n`, `>= n`, `< n`, `<= n` on Text, or the reversed literal comparison | one lower / one upper length bound; Unicode scalars |

For Number bounds, `n` is a Number literal, including a signed literal. For Text length bounds it must be a
non-negative whole literal; a negative or fractional literal bound is `VL0209`. A computed Text length bound
stays a residual filter condition.
Parentheses do not change extraction. Normalize a reversed literal comparison by swapping operands and reversing
its operator (`0 <= value` becomes `value >= 0`; likewise Text length). Such bounds count toward the same duplicate
lower/upper-bound rule. Computed Number or Text length bounds, `==`, negation, implication, disjunction, patterns and record-field
relations remain residual conditions. No other algebraic rewriting or constraint solving occurs. Duplicate bound
directions or whole conjuncts are `VL0209` even if redundant. A Text length interval is intersected with nonnegative
integers; a Number whole interval with integers. Missing endpoints retain the base's limits. Number bounds / whole
on a non-Number, and length bounds on a non-Text domain, are `VL0209`.

**R-CHK-15** Membership requires the closed set or complete `where` condition. The complete condition is retained
in HIR and evaluated once in source order; extraction is for generation/static simple checks, not a rewrite of
membership evaluation. Residual conjuncts are excluded from the artifact IR `domains` table (R-IR-24), and evaluated
by `velme-check` from that HIR, using the same validated lowering and reference interpreter as R-CHK-11. This
evaluation is used for generated-value filtering, the scarcity probe, and input/output membership even when a
WASM body produced the value. A predicate returning false or raising a value error (e.g. `VL0602`) means
"not a member"; input/output reports retain that cause. Budget exhaustion keeps its budget code (R-CHK-08),
and an internal error remains `VL0607`. Conditions are never LLM-judged (INV-2).

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-CHK-01 | `- result + 1` (a `Number`) as a check item yields `VL0204`. |
| AC-CHK-02 | A check using a `call` binding (`result.score == score`) type-checks; using an undefined name yields `VL0202`. |
| AC-CHK-03 | `- CalculateScore(player) > 0` in a check yields `VL0303`. |
| AC-CHK-04 | For `BuildPlayerSummary` returning `rank: 3` when `rank = 4`, the report shows the assertion `result.rank == rank`, Expected `4`, Received `3`, and the binding values. |
| AC-CHK-05 | For an input `xs: List<Number>` given `[]`, `every x in xs has false` is true and `some x in xs has true` is false (a bare `[]` has no element type, R-TYP-13). |
| AC-CHK-06 | `every b in result has b.bounce >= 5` failing on the third element reports index 2 and that element's value, and does not evaluate later elements. |
| AC-CHK-07 | `if players is empty then result is empty` holds when `players` is non-empty regardless of `result`; its `then` side is reported "not evaluated". |
| AC-CHK-08 | A check `- total / count > 1` with `count = 0` fails with `VL0501` citing cause `VL0602`; the goal's output is not returned. |
| AC-CHK-09 | Two failing items are both reported in source order; the trace cites the first. |
| AC-CHK-10 | A check that exhausts fuel yields `VL0601`, not `VL0501`. |
| AC-CHK-11 | The same failing check produces an identical report (values, index, order) on the interpreter and the WASM backend. |
| AC-CHK-12 | Quantifier variable `player` inside a goal with parameter `player` yields `VL0306`. |
| AC-CHK-13 | `is whole` and its negation agree with fractional-part semantics for positive, negative and zero Numbers; nullable use needs narrowing, non-Number use is `VL0206`; lowered checks validate (D2). |
| AC-CHK-14 | A domain condition sees only `value` and check-local names; a record condition relating two fields passes; a goal input, call binding or `result` referenced there yields `VL0209` (D2). |
| AC-CHK-15 | Top-level conjunction extraction finds Number bounds/whole and Text length bounds, including reversed literal comparisons; forward plus reversed bounds in the same direction yield `VL0209`. Computed Number and Text length bounds (e.g. `value.length <= other_expr`), `or` and record relations stay residual; only negative/fractional literal Text length bounds yield `VL0209` (D2). |
| AC-CHK-16 | A residual condition returning false or dividing by zero rejects membership with its cause; fuel exhaustion remains `VL0601`; interpreter/WASM body execution uses the same HIR predicate evaluator (D4). |
