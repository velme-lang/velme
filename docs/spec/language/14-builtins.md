# 14 — Built-in Functions

**Status:** v0.1 · **Area:** BLT
**Read when:** adding or changing a built-in, implementing one in a backend, writing the builtins section of the synthesis prompt, or bumping `builtins_version`.
**Depends on:** [SPEC](../SPEC.md), [11-types](11-types.md), [13-check-dsl](13-check-dsl.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-22, D-36)
**Source:** §7.4, §14, §16, §27, §32

## 1. Purpose & boundaries

Built-ins are the only operations available to synthesized IR beyond operators, records and lists, and the only
functions a check may call. They are pure, deterministic and total except for the listed errors (INV-3, INV-4). The IR
node shapes that invoke them are in [21-ir](../compiler/21-ir.md); host-function exposure in the WASM sandbox is in
[31-wasm-sandbox](../runtime/31-wasm-sandbox.md).

**R-BLT-01** The catalog below is the single source of truth, defined once as data in `velme-builtins` and consumed by
sema, the IR validator, the synthesis prompt, the interpreter and the WASM backend (CC-CONST). A name not in the catalog
is `VL0202` in a check and `VL0402 IRInvalid` in IR.

**R-BLT-02** "checks" = callable from `check` items (and lowered checks); "IR" = callable from synthesized IR. Nothing is
callable from a `call` block (R-GOAL-08).

## 2. Value built-ins

| Name | Signature | checks | IR | Behaviour / errors |
|---|---|---|---|---|
| `length` | `(List<T>) -> Number`, `(Text) -> Number` | ✓ | ✓ | surface `x.length`; Text counts Unicode scalar values |
| `is_empty` | `(T?) / (List<T>) / (Text) -> Boolean` | ✓ | ✓ | surface `is empty` |
| `maximum` | `(List<Number>) -> Number?` | ✓ | ✓ | `nothing` for `[]` |
| `minimum` | `(List<Number>) -> Number?` | ✓ | ✓ | `nothing` for `[]` |
| `sum` | `(List<Number>) -> Number` | ✓ | ✓ | `0` for `[]`; strict left-to-right addition; overflow `VL0602` |
| `contains` | `(List<T>, T) -> Boolean` | ✓ | ✓ | structural equality (R-TYP-20) |
| `abs` | `(Number) -> Number` | ✓ | ✓ | |
| `floor`, `ceil` | `(Number) -> Number` | ✓ | ✓ | |
| `round` | `(Number) -> Number` | ✓ | ✓ | ties away from zero (`2.5 → 3`, `-2.5 → -3`) |
| `clamp` | `(x: Number, low: Number, high: Number) -> Number` | ✓ | ✓ | `low > high` → `VL0602` |
| `concat` | `(Text, Text) -> Text` | ✓ | ✓ | |
| `to_text` | `(Number) -> Text` | ✓ | ✓ | plain decimal rendering (R-TYP-08) |
| `range` | `(n: Number) -> List<Number>` | ✓ | ✓ | `[0, 1, …, n-1]`; `n` integer-valued and `≥ 0` else `VL0602`; `n` above the list limit → `VL0606` |
| `random` | `(seed: Number, index: Number) -> Number` | ✓ | ✓ | §3 |

**R-BLT-03** Remainder/modulo, transcendental functions (`sqrt`, `sin`, `pow`, `log`) and Text
ordering/searching are not in v0.1 (D-36). Adding one is a `builtins_version` bump (§5) and, for any whose decimal
result must be rounded, a language decision on the rounding rule.

## 3. `random` (D-22)

**R-BLT-04** `random(seed, index)` is the `(index + 1)`-th output of SplitMix64 seeded with `seed`, mapped to `[0, 1)` with 18 decimal places.
All arithmetic is wrapping unsigned 64-bit:

```text
GOLDEN = 0x9E3779B97F4A7C15
mix(z):
    z = (z XOR (z >> 30)) * 0xBF58476D1CE4E5B9
    z = (z XOR (z >> 27)) * 0x94D049BB133111EB
    return z XOR (z >> 31)

s      = seed as signed 64-bit, reinterpreted as unsigned 64-bit (two's complement)
i      = index as unsigned 64-bit
z      = s + (i + 1) * GOLDEN
r      = (mix(z) as u128 * 10^18) >> 64   # integer in [0, 10^18)
random = r × 10^-18                       # exact Number (scale 18), in [0, 1)
```

**R-BLT-05** `seed` must be integer-valued (R-TYP-07); `index` must be integer-valued and `≥ 0`. Otherwise
`VL0602 ArithmeticError`. There is no zero-argument `random()` and no runtime-supplied seed in v0.1 (D-22): a goal
that needs randomness takes a `seed` parameter.

**R-BLT-06** Reference vectors (binding on every backend):

| seed | index | `mix(z)` | `random` |
|---|---|---|---|
| 0 | 0 | `0xe220a8397b1dcdaf` | `0.8833108082136426` |
| 0 | 1 | `0x6e789e6aa1b965f4` | `0.43152799704850997` |
| 42 | 0 | `0xbdd732262feb6e95` | `0.7415648787718233` |
| 42 | 7 | `0xccf635ee9e9e2fa4` | `0.8006318767135033` |
| -1 | 0 | `0xe4d971771b652c20` | `0.8939429202831845` |

A plan such as "create `count` bounce strengths from 5 through 10 using the seed" is synthesized as
`map(range(count), i -> 5 + random(seed, i) * 5)`.

## 4. Collection primitives (IR only)

These take an IR lambda ([21-ir](../compiler/21-ir.md)) and are how synthesized code iterates — learners never write
loops (§14). Lambdas are non-recursive and may read enclosing inputs, locals and lambda parameters.

| Name | Signature | Behaviour |
|---|---|---|
| `map` | `(List<T>, T -> U) -> List<U>` | in order |
| `filter` | `(List<T>, T -> Boolean) -> List<T>` | keeps order |
| `find` | `(List<T>, T -> Boolean) -> T?` | first match, else `nothing` |
| `reduce` | `(List<T>, U, (U, T) -> U) -> U` | left fold from the initial value |
| `sort_by` | `(List<T>, T -> Number, descending: Boolean) -> List<T>` | **stable**; keys compared as Numbers |
| `all` | `(List<T>, T -> Boolean) -> Boolean` | short-circuits at first `false`; `true` for `[]` (lowers `every`) |
| `any` | `(List<T>, T -> Boolean) -> Boolean` | short-circuits at first `true`; `false` for `[]` (lowers `some`) |

**R-BLT-07** Element visit order is list order for every primitive; a failing lambda (`VL0602`) fails the whole
primitive at the first failing element in that order. Every element visit consumes fuel
([30-execution-vibevm](../runtime/30-execution-vibevm.md)); result lists are subject to the list-size limit (`VL0606`).

**R-BLT-08** `Group` (§14) needs maps and is Future. Counting is `length(filter(…))`; no separate `count`.

## 5. `builtins_version`

**R-BLT-09** The catalog has a version `builtins_version` (`MAJOR.MINOR`, starting `0.1`), recorded in every artifact
manifest and in the synthesis cache key (D-11, INV-8).

| Change | Version effect |
|---|---|
| add a built-in | MINOR bump — existing artifacts stay valid |
| change any observable behaviour, signature or error of an existing built-in (including float formatting or rounding) | MAJOR bump — artifacts built against the old version are re-verified before reuse |
| remove a built-in | only with a new language version (P-1) |
| fix an implementation bug so a backend matches this spec | no bump; differential tests ([51-testing-quality](../delivery/51-testing-quality.md)) must catch the mismatch first |

**R-BLT-10** The synthesis prompt lists exactly the "IR" built-ins of the current version, with signatures
([22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md)).

## 6. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-BLT-01 | `random` returns the §3 reference vectors bit-exactly in the interpreter and the WASM backend. |
| AC-BLT-02 | `random(1.5, 0)` and `random(1, -1)` fail with `VL0602`. |
| AC-BLT-03 | `maximum([])` is `nothing`; `sum([])` is `0`; `maximum([3, 9, 2])` is `9`. |
| AC-BLT-04 | `round(2.5) == 3`, `round(-2.5) == -3`, `round(2.4) == 2` on every backend. |
| AC-BLT-05 | `range(3) == [0, 1, 2]`; `range(-1)` yields `VL0602`; `range` above the list limit yields `VL0606`. |
| AC-BLT-06 | `sort_by` keeps the original order of equal keys, in both directions. |
| AC-BLT-07 | `find` on a list with two matches returns the first; on no match returns `nothing`. |
| AC-BLT-08 | IR calling an unknown built-in, or `map` from a check, is rejected (`VL0402` / `VL0202`). |
| AC-BLT-09 | `to_text(820) == "820"`, `to_text(0.1) == "0.1"`, `to_text(-0) == "0"`. |
| AC-BLT-10 | Changing `builtins_version` changes every goal's synthesis cache key; `velme run --locked` rejects artifacts built on an older MAJOR with `VL0702 LockStale`. |
| AC-BLT-11 | `clamp(5, 10, 1)` yields `VL0602`; `contains([Player(…)], same Player(…))` is `true`. |
