# 12 — Goals, Calls & the Call DAG

**Status:** v0.1 · **Area:** GOAL
**Read when:** changing goal declarations, call-block checking, the call graph, cycle detection, `budget` or `examples`, or deciding what gets synthesized.
**Depends on:** [SPEC](../SPEC.md), [10-syntax-grammar](10-syntax-grammar.md), [11-types](11-types.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-4, D-5, D-7, D-8, D-9, D-18)
**Source:** §4, §6, §8, §8.1, §9, §10, §11, §12, §13, §14, §17, §44, §45, §46, §52

## 1. Purpose & boundaries

Defines what a goal is, how goals use other goals, and the static call graph. How the DAG is scheduled at run time is
in [30-execution-vibevm](../runtime/30-execution-vibevm.md); what the LLM receives is in
[22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md); `check` semantics are in [13-check-dsl](13-check-dsl.md).

## 2. Goal anatomy

```text
goal Name(param: Type, ...) -> OutputType:
    budget cpu=10ms memory=4mb        # optional (§6)
    call:                             # optional: which goals it uses, and with what
        binding = OtherGoal(param)
    plan: |                           # the intent, in plain language
        ...
    check:                            # deterministic assertions on the result
        - ...
    examples:                         # concrete input → expected output
        - Name(...) == ...
```

| Kind | `call` block | `result` binding | `plan` | What is synthesized |
|---|---|---|---|---|
| **Leaf** | none | — | required | the whole body, from inputs (IR, no calls) |
| **Composite** | yes | none | required | only the **tail**: an IR expression over inputs and bindings (D-5) |
| **Wired** | yes | yes | optional (documentation) | nothing — output is the `result` binding (D-4) |

**R-GOAL-01** A goal needs a non-empty `plan`, a `result` binding, or both. Otherwise `TL0307 GoalHasNoBody`
("tell Thela what this goal should do in `plan:`"). Bodiless signature-only goals do not exist in v0.1.

**R-GOAL-02** Goal names are unique in the file (`TL0203`) and share the declaration namespace with types
([11-types](11-types.md) R-TYP-19). Parameters are unique within a goal (`TL0203`).

**R-GOAL-03** Every v0.1 goal is pure: its output depends only on its inputs and the goals it calls (INV-4). `pure` is
reserved for the explicit form (D-8).

## 3. The `call` block

**R-GOAL-04** A goal may invoke another goal **only** through a binding in its own `call` block (INV-6). Plan text is
never scanned for goal names; since synthesized IR may not contain calls (D-5), a plan saying "call SecretFunction"
cannot cause a call.

| Rule | Violation | Code |
|---|---|---|
| **R-GOAL-05** The callee is a goal declared in this file. | `score = Unknown(x)` | `TL0301 UnknownGoal` |
| **R-GOAL-06** Argument count equals the callee's parameter count. | | `TL0302 CallArityMismatch` |
| **R-GOAL-07** Each argument is assignable to its parameter type (R-TYP-20). Message: "Cannot call CalculateScore: expected Player, received Text". | `CalculateScore("hello")` | `TL0204 TypeMismatch` |
| **R-GOAL-08** An argument is a path (input, earlier binding, and field accesses on them) or a literal — no arithmetic, no built-ins. Computation belongs in a goal (P-3). A type name used as a callee is also rejected here. | `s = Score(x + 1)` | `TL0303 InvalidCall` |
| **R-GOAL-09** A binding may reference only inputs and bindings defined on **earlier lines** (§12.5). | `b = Second(a)` above `a = First(x)` | `TL0305 BindingUsedBeforeDefinition` |
| **R-GOAL-10** Binding names are unique and do not reuse a parameter name. | | `TL0306 DuplicateBinding` |
| **R-GOAL-11** The whole-file goal graph is acyclic, including self-calls and mutual recursion. The diagnostic prints the cycle path `A → B → A`. | `value = A(x)` inside `A` | `TL0304 CallCycle` |

**R-GOAL-12** A binding's type is the callee's output type. A binding named `result` makes the goal **wired**; its type
must be assignable to the goal's output (`TL0204`).

**R-GOAL-13** Every binding is **required**: it executes even if the tail doesn't use it, and if it fails the goal
fails ([30-execution-vibevm](../runtime/30-execution-vibevm.md), D-9). `fallback`, `retry` and `optional` calls are
Future (D-24).

## 4. The call DAG

**R-GOAL-14** For each goal the compiler builds a DAG whose nodes are bindings and whose edges are references between
them. Inputs are wave 0; `wave(b) = 1 + max(wave(d) for each binding d that b references)` (0 if none). The tail, or
the `result` binding, completes the goal.

**R-GOAL-15** Source order does not imply execution order: any binding whose dependencies are complete may run,
concurrently with others (§9). Source order **does** fix the order of traces, explanations and failure selection (D-9).

Parallel (`BuildPlayerSummary`): all three bindings are wave 1.

```text
goal BuildPlayerSummary(player: Player) -> PlayerSummary:
    call:
        score = CalculateScore(player)
        rank = CalculateRank(player)
        badge = FindBadge(player)
    plan: |
        Combine the score, rank, and badge into one player summary.
    check:
        - result.score == score
        - result.rank == rank
        - result.badge == badge
```

Sequential (`BuildReceipt`): `order` wave 1 → `total` wave 2 → `receipt` wave 3. Mixed (`CreateLevelSummary`, §11 of
the original): `enemies`, `treasures`, `score` wave 1; `difficulty`, `reward` wave 2; tail last.

**R-GOAL-16** `thela explain` renders waves as "First / At the same time / Finally" directly from this DAG, with no LLM
call ([40-cli](../tooling/40-cli.md)).

**R-GOAL-17** Conditional calls (`when`), `choose`, loops and recursion are Future (P-5). Iteration inside a goal is
expressed by collection primitives in IR ([14-builtins](14-builtins.md) §4), never by user syntax.

## 5. What is synthesized (D-5)

**R-GOAL-18** Leaf goal: the LLM returns an IR body over the parameters. Composite goal: the LLM returns a tail IR
expression over the parameters and bindings (bindings appear as `Local`s with their types). In both cases the IR may use
whitelisted built-ins and must not contain goal calls; the validator rejects them (`TL0402 IRInvalid`). Wired goals
make no LLM request.

**R-GOAL-19** The LLM sees only child **signatures**, never child implementations (D-11).

## 6. `budget` (D-8)

| Key | Unit | Meaning |
|---|---|---|
| `cpu` | `ms`, `s` | CPU allowance for one invocation of this goal including its calls (converted to fuel + watchdog by [30-execution-vibevm](../runtime/30-execution-vibevm.md)) |
| `memory` | `kb`, `mb` (1 mb = 2^20 bytes) | peak memory for one invocation |
| `calls` | none | max goal invocations in this goal's subtree, itself included |
| `depth` | none | max call depth below this goal |

**R-GOAL-20** The effective limit is the minimum of the system cap, the caller's remaining allowance and the declared
value — a `budget` can only tighten (D-8). A declared value above the system cap, an unknown key, a wrong or missing
unit, a non-positive or non-integer count, or a repeated key is `TL0308 InvalidBudget`.

## 7. `examples` (D-7)

**R-GOAL-21** Each item is `Goal(literal, …) == literal` where `Goal` is the enclosing goal — the only place a goal
names itself. Calling any other goal is `TL0303`; argument/expected types follow R-TYP-20 (`TL0204`); arity `TL0302`.

**R-GOAL-22** Examples run before generated inputs during verification
([22-spellbook-synthesis](../compiler/22-spellbook-synthesis.md)) and by `thela test`. A mismatch is
`TL0502 ExampleFailed` showing the call, expected and received values. The goal's checks are also evaluated on every
example input.

## 8. Worked examples

### 8.1 Complete program

```text
type Player:
    name: Text
    jump_height: Number
    score: Number

type PlayerSummary:
    name: Text
    score: Number
    badge: Text

goal CalculateScore(player: Player) -> Number:
    plan: "Return the player's score."
    check:
        - result == player.score

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise.
    check:
        - result == "Gold" or result == "Silver" or result == "Bronze"
    examples:
        - FindBadge(Player(name: "Lina", jump_height: 3, score: 820)) == "Silver"
        - FindBadge(Player(name: "Tom", jump_height: 5, score: 1000)) == "Gold"

goal BuildPlayerSummary(player: Player) -> PlayerSummary:
    call:
        score = CalculateScore(player)
        badge = FindBadge(player)
    plan: |
        Build a player summary using the player's name,
        calculated score, and badge.
    check:
        - result.name == player.name
        - result.score == score
        - result.badge == badge
```

### 8.2 Nullable result (corrects F-3)

```text
goal FindHighestJumpingPlayer(players: List<Player>) -> Player?:
    plan: |
        Look at all the players.
        Find the one whose jump_height is the biggest.
        Return that player.
    check:
        - if players is empty then result is empty
        - if players is not empty then result is not empty
        - if result is not empty then every p in players has result.jump_height >= p.jump_height
```

### 8.3 Wired goal (legal form of the original Test 2)

```text
goal Double(x: Number) -> Number:
    plan: "Multiply x by 2."
    check:
        - result == x * 2

goal AddOne(x: Number) -> Number:
    plan: "Add 1 to x."
    check:
        - result == x + 1

goal Main(x: Number) -> Number:
    call:
        doubled = Double(x)
        result = AddOne(doubled)
    plan: "Double x, then add one."
    examples:
        - Main(4) == 9
```

`Main` makes no LLM request; its plan is documentation.

## 9. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-GOAL-01 | A goal with neither `plan` nor `result` binding yields `TL0307`. |
| AC-GOAL-02 | `score = CalculateScore("hello")` with parameter type `Player` yields `TL0204` with "expected Player, received Text", before any synthesis. |
| AC-GOAL-03 | `value = A(x)` inside goal `A` yields `TL0304`; `A → B → A` yields `TL0304` printing that path. |
| AC-GOAL-04 | `b = Second(a)` written above `a = First(x)` yields `TL0305`, even though a topological order exists. |
| AC-GOAL-05 | `s = Score(x + 1)` yields `TL0303`; `s = Score(player.stats)` type-checks. |
| AC-GOAL-06 | A call to an undeclared goal yields `TL0301`; a wrong argument count yields `TL0302`. |
| AC-GOAL-07 | For §4 `CreateLevelSummary`, the computed waves are `{enemies, treasures, score}`, `{difficulty, reward}`. |
| AC-GOAL-08 | §8.3 `Main` builds and runs with the `scripted` provider receiving zero requests for `Main` and passes `Main(4) == 9`. |
| AC-GOAL-09 | Synthesized IR for a composite goal that contains a goal call is rejected with `TL0402`. |
| AC-GOAL-10 | `budget cpu=10ms calls=999` where the system call cap is 128 yields `TL0308`; `budget depth=2` lowers the effective depth to 2. |
| AC-GOAL-11 | An example calling a different goal yields `TL0303`; a wrong expected value at run time yields `TL0502` showing expected and received. |
| AC-GOAL-12 | A binding named `result` whose type isn't assignable to the goal's output yields `TL0204`. |
| AC-GOAL-13 | The §8.1 and §8.2 programs pass `thela check` with no diagnostics. |
