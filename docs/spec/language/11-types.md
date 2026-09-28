# 11 — Types

**Status:** v0.1 · **Area:** TYP
**Read when:** changing type checking, assignability, equality, `Number` arithmetic, narrowing, or the JSON value mapping.
**Depends on:** [SPEC](../SPEC.md), [10-syntax-grammar](10-syntax-grammar.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-3, D-6, D-23, D-36)
**Source:** §5, §5.1, §5.2, §5.3, §31

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

**R-TYP-13** `List<T>` is invariant: `List<Number>` is not assignable to `List<Number?>`. An empty list literal `[]`
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

## 8. Assignability, equality and ordering

**R-TYP-20** A value of type `S` is assignable to `T` iff `S = T`, or `T = U?` and (`S = U` or `S = Nothing` or
`S = U?`). There is no other implicit conversion. Assignability applies to call arguments, bindings to outputs,
record-literal fields, example values and JSON inputs.

| Operator | Operands | Result |
|---|---|---|
| `==` `!=` | same type, or one assignable to the other | `Boolean`; structural for lists (length + elementwise) and records (fieldwise); `nothing == nothing`; a present value compared with `nothing` is `false` for `==`, `true` for `!=` (D-61) |
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

Narrowing reaches into nested expressions (including quantifier bodies) inside that scope. It does not flow through
`not`, through built-in calls (`maximum(xs)` stays `Number?`), or across separate check items.

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

**R-TYP-23** Encoding is canonical: records emit fields in declaration order, numbers per R-TYP-08. Hashing uses the
canonical form of D-21 (sorted keys) — a separate, internal encoding.

## 11. Limits

**R-TYP-24** Runtime value limits — list length, total output bytes — are budget items owned by
[30-execution-vibevm](../runtime/30-execution-vibevm.md); exceeding one is `VL0606 SizeLimitExceeded`. Input decoding
applies the same limits before a goal starts.

## 12. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-TYP-01 | `CalculateScore("hello")` where the parameter is `Player` yields `VL0204` with "expected Player, received Text". |
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
