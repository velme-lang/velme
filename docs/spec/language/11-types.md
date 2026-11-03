# 11 — Types

**Status:** v0.2 · **Area:** TYP
**Read when:** changing type checking, assignability, equality, `Number` arithmetic, narrowing, or the JSON value mapping.
**Depends on:** [SPEC](../SPEC.md), [10-syntax-grammar](10-syntax-grammar.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-3, D-6, D-23, D-36)

## 1. Purpose & boundaries

Defines the v0.1 type system and the value model shared by the checker, the IR ([21-ir](../compiler/21-ir.md)), the
interpreter and the WASM backend. Operator syntax is in [10-syntax-grammar](10-syntax-grammar.md); check-specific
rules in [13-check-dsl](13-check-dsl.md); built-in signatures in [14-builtins](14-builtins.md).

**R-TYP-01** Every goal parameter and output has an explicit type (D-3). Missing types are `VL0101` (a parse error, since
the grammar requires them); the help line shows the fix using the author's own name, e.g. "write a type after
`players`, like `players: List<Player>`".

## 2. Type catalog

| Type | Values | Written |
|---|---|---|
| `Number` | exact decimal, 28 fractional digits max, no `-0` (§3) | `3`, `2.5`, `-1` |
| `Text` | sequence of Unicode scalar values | `"Lina"` |
| `Boolean` | `true`, `false` | |
| `Nothing` | the single value `nothing` | only as part of `T?` or the literal |
| `T?` | `T` or `nothing` — i.e. `T \| Nothing` | `Player?` |
| `List<T>` | ordered, finite, immutable | `[1, 2]`, `List<Player>` |
| record | named, fixed set of typed fields | `Player(name: "Lina", jump_height: 3, score: 820)` |
| domain | named subset of Text, Number, or a declared record (§7.1) | `Age`, `Risk`, `Offer` |

**R-TYP-02** Not in v0.1 (P-5): generics beyond `List`, maps, enums, tuples, user ADTs, inheritance, interfaces,
function types, a binary-float `Float` type (D-36). The names are not reserved except where D-24 says so.

**R-TYP-03** `Nothing` alone as a parameter, field or output type, and `Nothing?`, are `VL0204 TypeMismatch` ("a value
that can only be `nothing` carries no information").

## 3. `Number` (D-36)

A `Number` is an exact decimal `c × 10^-e`: integer coefficient `|c| < 2^96` (about 7.9 × 10^28) and scale
`0 ≤ e ≤ 28`. It is one type with one meaning in every program and backend; it is not configurable (D-36).

**R-TYP-04** `+ - *` and unary `-` are exact when the exact result fits; otherwise it is rounded half-to-even to the
largest scale that fits. `/` gives the quotient rounded half-to-even the same way, so `1 / 3` is
`0.3333333333333333333333333333` and `1 / 3 * 3` is `0.9999999999999999999999999999`. Evaluation order is left to
right as parsed; no reassociation, in any backend (INV-3).

**R-TYP-05** Division by zero, and a result whose integer part does not fit, fail with `VL0602 ArithmeticError`,
naming the operation and operands.

**R-TYP-06** A result of `-0` is normalized to `0` before it is stored, compared, hashed or output.

**R-TYP-07** "Integer-valued" (required by some built-ins) means no fractional part. Built-ins that take a 64-bit
integer (`random`) also require `-2^63 ≤ x < 2^63`, else `VL0602`.

**R-TYP-08** Equality and ordering are numeric: `0.1 + 0.2 == 0.3` is `true` and `2.50 == 2.5`. Rendering (output,
`to_text`, canonical JSON) is plain decimal: no exponent, trailing fractional zeros removed, integer-valued numbers
without a fraction (`820`, not `820.0`; `2.50` renders `2.5`).

## 4. `Text` and `Boolean`

**R-TYP-09** `Text` supports `==`, `!=`, `is empty`, `.length` (count of Unicode scalar values) and the text built-ins.
Ordering (`<` …) on `Text` is `VL0206 InvalidOperandType` in v0.1. No normalization (NFC etc.) is applied: equality
compares scalar values.

**R-TYP-10** `Boolean` supports `==`, `!=`, `and`, `or`, `not`. There is no truthiness: `if score then …` is
`VL0204`.

## 5. Nullable types and `Nothing`

**R-TYP-11** `T?` is a union of `T` and `Nothing`. `(T?)?` cannot be written (`VL0101`); `List<T?>` and `List<T>?` are
distinct and both allowed.

**R-TYP-12** Field access, arithmetic, ordering, projection and quantification on an operand of type `T?` are
`VL0207 NullableAccess` unless the operand is narrowed (§9). `==`/`!=` against `T?` are always allowed.

**R-TYP-25** On an **optional collection** — `List<T>?` or `Text?` — `is empty` is `true` for `nothing` and for a
present but empty value (`[]` / `""`); `is not empty` is its negation and narrows the operand to the non-optional
`List<T>` / `Text` (§9). This differs from a bare `T?` for a non-collection `T`, where only `nothing` counts as empty
(D-60); the `is_empty` IR node ([compiler/21](../compiler/21-ir.md)) implements the same rule.

## 6. Lists

**R-TYP-13** `List<T>` is invariant, except for domain widening in R-TYP-30: `List<Number>` is not assignable to `List<Number?>`. An empty list literal `[]`
takes its element type from the expected type; with no expected type it is `VL0204` ("can't tell what kind of list
this is").

**R-TYP-26** A non-empty list literal's element type, absent a wider expected type, is the **least common type** of
its elements under assignability (R-TYP-20): `[1, nothing]` is `List<Number?>`. Elements with no common type (e.g.
`[1, "a"]`) are `VL0204`.

**R-TYP-14** On `List<R>` where `R` is a record, `xs.field` is a **projection** of type `List<F>` (D-6); chains
`xs.a.b` project through nested records. `.length` on a list is always the list's length, even if `R` has a field
named `length`. Projection over `List<R?>` is `VL0207`.

## 7. Records

**R-TYP-15** Record types are **nominal**: `Player` and `Summary` are different types even with identical fields.
Record *values* compare structurally (§8).

**R-TYP-16** A record has at least one field (the grammar requires one, so `type Empty:` is `VL0101`, `language/10`
§4); field names are unique within it (`VL0203 DuplicateDeclaration`); an
unknown field in access or a literal is `VL0205 UnknownField`.

**R-TYP-17** A record literal names every field exactly once, in any order; a missing field is `VL0204` listing the
missing names. Nullable fields are not implicitly `nothing` (P-3).

**R-TYP-18** Recursive types — a record reaching itself through fields, `List`, or `?`, directly or via other records —
are `VL0208 RecursiveType`, reporting the cycle path.

**R-TYP-19** Type and goal names share one declaration namespace per file and may not shadow built-in type names
(`VL0203`). Unknown type names are `VL0201 UnknownType`.

### 7.1 Domain types (D-151, D-158, D-159)

**R-TYP-28** A domain is nominal and immutable. `Text one of …` is a closed set of exact Text values, compared
without Unicode normalization; declaration order is retained for generation. `Number where …` and `Text where …`
constrain their base with a Boolean condition on `value`. A record declaration with `where` constrains the whole
record and may relate its fields. Its underlying record keeps the declaration's name and fields. In checks and
conditions, a constructor yields that underlying record; it does not assert domain membership. Membership is
enforced only at inputs, outputs and source-literal positions (R-TYP-31, R-RUN-26, R-RUN-28). These are the only domain bases: no bare aliases, domains of domains, Boolean, List or
optional bases. A field or item may itself have a domain type, and a domain may be used inside `List` and `?`.
Arithmetic on Number domains produces `Number`, never a domain. There is no static exhaustiveness check in 0.2;
the generated suite covers closed-set members up to 16 members (R-SYNTH-64).

**R-TYP-29** An invalid declaration is `VL0209 InvalidDomain`: an unsupported base (including `type Adult = Person where …`); a closed set with no members,
a non-Text member or an exactly duplicate member; a condition that is not a well-typed Boolean in R-CHK-13's scope;
a Number bound or `is whole` applied to the wrong base; a negative or fractional literal Text length bound; more than one extracted lower bound, upper bound or
`is whole` conjunct; a simple interval with no representable value; or exceeding R-TYP-33's declaration limits.
The diagnostic names the domain, offending clause and reason (and the underlying name/type error, if any). Empty
intervals include equal endpoints with either end exclusive, no integer in a whole interval, and no nonnegative
integer Text length. Conditions the compiler cannot reduce to simple constraints are valid filters, not invalid
declarations; their scarcity is diagnosed separately by R-SYNTH-65.

## 8. Assignability, equality and ordering

**R-TYP-20** A value of type `S` is assignable to `T` iff `S = T`, or `T = U?` and (`S = U` or `S = Nothing` or
`S = U?`), or the domain rules below apply. Assignability applies to call arguments, bindings to outputs,
record-literal fields, example values and JSON inputs.

**R-TYP-30** A domain `D` is usable as its base `B`, including recursively under matching `List` and `?`
constructors: `List<Age?>` widens to `List<Number?>`. This covariance is an explicit, limited exception to
R-TYP-13; it does not make `List<Number>` assignable to `List<Number?>` in source calls. A direct expression of a
different domain is rejected where `D` is expected: `VL0204` in source, `VL0402` in IR. Source calls still reject
non-literal base-typed arguments (R-TYP-31); synthesized bodies use D-163's recursive `compat` relation (R-IR-25).

Static types preserve domain identity through `input`, `local`, `field`, `let`, `if`, `unwrap_or`, `filter`, `find`
and `sort_by` when their selected/unified value types retain it. Type-variable builtins instantiate to that domain;
`binary`/`unary` operators and fixed-signature builtins return their base result types. Unifying distinct domains,
or a domain and its base, yields the base, recursively through containers. Equality and other operators first
widen their operands, so `age == score` for distinct Number domains is valid base equality, subject to R-TYP-32.
Static domain-to-domain rejection is best effort on direct expressions: a join can lose identity. Deep `VL0504`
output checking is the sound guarantee (D-163), not a static proof of membership.

**R-TYP-31** An expected-type pass checks source literals in domain positions (call arguments, record fields,
list items and examples), recursively through records, lists and optional values. Signed Number literals use their
signed value, not the unsigned token. A closed-set
non-member or a literal violating extracted Number bounds, `is whole`, or Text length bounds is `VL0204`, naming
the domain and failed constraint: "`{value}` isn't a valid `{domain}`: {reason}." A non-literal base-typed source call argument is not narrowed implicitly; it is
`VL0204`, with help "declare `X`'s output as `Age`" when `X` supplies the base-typed argument expected as `Age`.
A synthesized base-valued expression is accepted in a domain output, record field or list item by
R-IR-25, with membership checked at the goal's output (R-RUN-26); no cast node is inserted. Residual predicates
are evaluated by `velme-check` from HIR (R-CHK-15), not by the literal bounds pass. After that pass,
`velme-check` checks full membership of domain-typed source literals (including nested example/call-argument
literals) with the HIR evaluator; false or a value error is `VL0204` with the domain and cause. These source
literals are fully constant by `language/10` §4's literal grammar, so this is a source contract check, not IR residual
validation or a run-time cast. Use fresh system budgets per literal; budget/internal failures keep their codes.
This makes source literal arguments members before inter-goal execution, without a call-boundary check.
The constant record and list literals inside a `check` item or a domain condition are domain positions for their
domain-typed fields and items too (D-171): both passes apply, reported as `VL0204`, or `VL0209` inside a condition (a budget or internal failure keeps its own code there too;
the simple pass sees every domain's extracted constraints whichever is declared first). A
record-domain constructor there still yields its underlying record and asserts no membership of its own (R-TYP-28).
A list item that is a direct value of a different domain than the list's element is `VL0204` ("Expected Age, but got
Score."), never widened to the base by a join (R-TYP-30, D-163).
The source expected-type traversal supplies each constant to the predicate evaluator; it does not invoke the
runtime membership walk (R-RUN-27).

**R-TYP-32** In `==` or `!=`, a closed-set domain compared with a Text literal outside its members is rejected,
in either operand order: `VL0204` in source, `VL0402` in IR. A member literal is accepted and uses ordinary Text
equality. Comparing to a computed Text remains ordinary base equality; this rule is not an exhaustiveness proof.

| Operator | Operands | Result |
|---|---|---|
| `==` `!=` | widen domain operands to their bases first; then same type, or one assignable to the other (R-TYP-32 still rejects non-member literals) | `Boolean`; structural for lists (length + elementwise) and records (fieldwise); `nothing == nothing`; a present value compared with `nothing` is `false` for `==`, `true` for `!=` (D-61) |
| `<` `<=` `>` `>=` | `Number`, `Number` | `Boolean` |
| `+ - * /`, unary `-` | `Number` | `Number` (R-TYP-04..06) |
| `and` `or` `not` | `Boolean` | `Boolean`, short-circuit |
| `is empty` / `is not empty` | `T?`, `List<T>`, `Text` | `Boolean` — `nothing`, `[]`, `""` are empty |

**R-TYP-21** Any other operand combination is `VL0206 InvalidOperandType`, with a hint where one exists (`+` on
`Text` → "use `concat`"; `is empty` on `Player` → "a `Player` is never empty — did you mean `Player?`").

## 9. Narrowing (D-6)

**R-TYP-22** A **path** is a name (input, binding, `result`, quantifier variable) followed by zero or more `.field`
accesses. Paths are immutable, so their type can be narrowed:

| Condition `c` on path `p: T?` | Narrowed to `T` in |
|---|---|
| `p is not empty` | right operand of `c and …`; `then` branch of `if c then …` |
| `p is empty` | right operand of `c or …` |
| `p != nothing` / `p == nothing` | same as `is not empty` / `is empty` |

Narrowing reaches into nested expressions (including quantifier bodies) inside that scope. It flows through `not`
(which swaps the sides, R-TYP-27) and through an `if` used as a condition (R-TYP-27, D-6), but not through built-in
calls (`maximum(xs)` stays `Number?`) or across separate check items.

**R-TYP-27** Compound conditions combine the narrowings of their operands: in `if a and b then X`, the narrowings `a`
and `b` each establish (per the table above) both apply within `X`. In `if a or b then X else Y`, the narrowings that
`not a` and `not b` establish both apply within `Y`. A leading `not c` swaps which side of `c`'s own narrowing applies,
per the table.

## 10. JSON value mapping (D-23)

One mapping is used by `velme run` input/output, `examples`, fixtures and IR literals ([21-ir](../compiler/21-ir.md)).

| Velme | JSON | Decoding rule |
|---|---|---|
| `Number` | number | decoded exactly from its text, never via binary float (exponents allowed); not exactly representable (R-TYP-04 range/scale) → `VL0902 InvalidInput`; `-0` → `0` |
| `Text` | string | lone surrogate escapes → `VL0902` |
| `Boolean` | `true` / `false` | |
| `Nothing` / `T?` | `null` | `null` for a non-nullable `T` → `VL0902` |
| `List<T>` | array | each element decoded as `T` |
| record | object | exactly the declared fields; unknown or missing keys → `VL0902` naming them |
| domain | its base's JSON | no tag or wrapper; membership checked on input (R-CLI-29) and output (R-RUN-26) |

**R-TYP-23** Encoding is canonical: records emit fields in declaration order, numbers per R-TYP-08. Hashing uses the
canonical form of D-21 (sorted keys) — a separate, internal encoding.

## 11. Limits

**R-TYP-24** Runtime value limits — list length, total output bytes — are budget items owned by
[30-execution-vibevm](../runtime/30-execution-vibevm.md); exceeding one is `VL0606 SizeLimitExceeded`. Input decoding
applies the same limits before a goal starts.

**R-TYP-33** A closed set has at most 1 000 members and at most 64 KiB of UTF-8 in total across its decoded
members (sum of their UTF-8 byte lengths, excluding syntax/JSON escaping). Exceeding either declaration limit is
`VL0209`. These are per-set limits; the serialized IR still obeys its separate 1-MiB cap (21 §7).
Domain predicates retain the source expression-depth and execution budgets of R-CHK-11 and R-RUN-25;
runtime values keep R-TYP-24's limits. A domain does not relax any base-type or budget limit. The maximum set
size is distinct from the 16-member generated coverage guarantee.

## 12. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-TYP-01 | `CalculateScore("hello")` where the parameter is `Player` yields `VL0204` "Expected Player, but got Text." |
| AC-TYP-02 | Passing a `Number` to a `Number?` parameter and `nothing` to a `Player?` parameter type-checks; passing `Number?` to `Number` yields `VL0204`. |
| AC-TYP-03 | `List<Number>` passed where `List<Number?>` is expected yields `VL0204`. |
| AC-TYP-04 | `result.jump_height` with `result: Player?` yields `VL0207`; `result is not empty and result.jump_height > 0` type-checks. |
| AC-TYP-05 | `type Node:` with field `next: Node?` yields `VL0208` naming `Node → Node`. |
| AC-TYP-06 | `1 / 0` evaluated at run time yields `VL0602`, in the interpreter and the WASM backend alike. |
| AC-TYP-07 | `0 * -1` renders `0`, and its canonical hash equals that of `0`. |
| AC-TYP-08 | `820` renders without a fraction. |
| AC-TYP-09 | `"b" < "a"` yields `VL0206`; `"a" + "b"` yields `VL0206` with a hint to use `concat`. |
| AC-TYP-10 | Two record values with equal fields compare `==` true; records of two types with identical fields cannot be compared (`VL0206`). |
| AC-TYP-11 | JSON input with an extra field for a record yields `VL0902` naming the field; `null` for a non-nullable field yields `VL0902`. |
| AC-TYP-12 | A record literal missing a nullable field yields `VL0204` listing it. |
| AC-TYP-13 | `players.jump_height` with `players: List<Player>` has type `List<Number>`; `players.length` is the list length. |
| AC-TYP-14 | Output of a `Player` record lists fields in declaration order. |
| AC-TYP-15 | `0.1 + 0.2 == 0.3` is `true`; `to_text(2.50)` is `"2.5"`; `1 / 3` renders `0.3333333333333333333333333333`; JSON input `0.1` round-trips to output `0.1` — interpreter and WASM alike. |
| AC-TYP-16 | JSON input `1e-29` or a number with more than 28 fractional digits yields `VL0902`; a goal parameter written without a type yields `VL0101` whose help names that parameter. |
| AC-TYP-17 | `type Bad: value: Nothing` and a field typed `Nothing?` both yield `VL0204`. |
| AC-TYP-18 | `type Empty:` with no fields yields `VL0101` (R-TYP-16); two fields both named `x` yield `VL0203`; accessing an undeclared field yields `VL0205`. |
| AC-TYP-19 | `type Number: …` (a type reusing a built-in type name) yields `VL0203`; a goal parameter typed `Unknown` yields `VL0201`. |
| AC-TYP-20 | With `a: Player?` and `b: Player?`, `if a is empty or b is empty then 0 else a.score + b.score` type-checks (both narrowed in the `else` branch, R-TYP-22, R-TYP-27); `if a is not empty and b is not empty then a.score + b.score else 0` also type-checks. |
| AC-TYP-21 | For `xs: List<Number>?`, `xs is empty` is `true` for `nothing` and for `[]`, and `false` for `[1]`; `xs is not empty` narrows `xs` to `List<Number>`. For `t: Text?`, `t is empty` is `true` for `nothing` and for `""`. |
| AC-TYP-22 | Text sets, bounded/whole Number domains, Text length domains and a record condition type-check; Number-domain arithmetic returns Number; no exhaustiveness diagnostic is emitted for a partial set comparison (D2). |
| AC-TYP-23 | Each invalid-declaration category in R-TYP-29 yields `VL0209`, including an empty set, duplicate Text, wrong base, non-Boolean condition, two lower bounds and an empty whole/length interval (D2). |
| AC-TYP-24 | `Age` widens to Number, `List<Age?>` to `List<Number?>`, and a record domain to its underlying record; invariant `List<Number>` to `List<Number?>` is still rejected (D2). |
| AC-TYP-25 | Direct distinct-domain source arguments are rejected (`VL0204`); a base-typed source call argument is also rejected with help to declare the producing goal's output as the expected domain. IR direct-expression rejection, node identity and joins are AC-IR-17, AC-IR-20–21 (D2). |
| AC-TYP-26 | The expected-type pass rejects non-member Text, out-of-bound Number, fractional whole and out-of-length Text literals, including nested fields/items/optionals, with `VL0204`, including signed Number literals; valid edges pass (D2). |
| AC-TYP-27 | A closed-set comparison to a non-member Text literal is `VL0204` for both `==`/`!=` and both operand orders; member literals and computed Text comparisons pass (D2). |
| AC-TYP-28 | Domain JSON values and canonical value hashes equal their base values, with no domain tag, including nested records, lists and `null` optionals (D4). |
| AC-TYP-29 | A set of 1 000 members totalling at most 65 536 UTF-8 bytes passes declaration limits; 1 001 members or 65 537 total bytes across multiple members yield `VL0209`. A single 65 536-byte member passes; aggregate IR and runtime budgets still apply (D2). |
| AC-TYP-30 | After simple expected-type checking, a source literal violating a residual Number/Text or cross-field record predicate is `VL0204` with its cause, including nested example/call literals; valid constants pass, value errors reject and budget/internal codes remain unchanged. No call-boundary runtime check is added (R-TYP-31). Tested by D4. |
| AC-TYP-31 | A record-domain constructor in a check/condition has the underlying record type and is not membership-checked there; the same value at an input/output/source-literal position is checked (R-TYP-28). Tested by D2 (typing), D4 (membership). |
| AC-TYP-32 | `type Adult = Person where …`, for a named record `Person`, is `VL0209`; a record domain uses `type Adult where …:` with its own indented fields (R-TYP-28–29). Tested by D2. |
