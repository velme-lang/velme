# 10 — Syntax & Grammar

**Status:** v0.1 · **Area:** SYN
**Read when:** writing or changing the lexer or parser, adding syntax, reading a parse diagnostic, or adding golden parser tests.
**Depends on:** [SPEC](../SPEC.md), [92-decisions-questions](../reference/92-decisions-questions.md) (D-7, D-8, D-18, D-19, D-24, D-25)
**Source:** §4, §6, §7, §8, §19, §48

## 1. Purpose & boundaries

Defines the characters, tokens, layout and grammar of a v0.1 `.velme` file. What a well-formed program *means* is in
[11-types](11-types.md), [12-goals-calls](12-goals-calls.md) and [13-check-dsl](13-check-dsl.md). Diagnostic rendering
is in [20-compiler-architecture](../compiler/20-compiler-architecture.md).

**R-SYN-01** A v0.1 program is exactly one UTF-8 file with extension `.velme` (D-18). A byte sequence that is not valid
UTF-8 is `VL0901 FileError`. A leading BOM is ignored, but spans still count its bytes, so every offset is a byte
offset into the file as saved (D-75).

**R-SYN-02** Line endings `\n` and `\r\n` are equivalent; a lone `\r` is `VL0101 UnexpectedToken`.

## 2. Lexical structure

### 2.1 Tokens

| Token | Form | Notes |
|---|---|---|
| `NAME` | ASCII letter or `_`, then letters, digits, `_` | ASCII only in v0.1; Unicode identifiers Future. Text and plans are full Unicode. A word with a non-ASCII letter is one `VL0101` (D-76). |
| `NUMBER` | `digits [ "." digits ]`, `_` allowed between digits | no exponent, no leading `.`, no sign (unary `-` is an operator). |
| `UNIT` | `ms` `s` `kb` `mb` written directly after a `NUMBER` (no space) | only valid inside `budget` (§4.5). |
| `VERSION` | `digits "." digits` | only valid after `language: velme/` in the header (§4); compared as text, not decoded as a `Number` (R-SYN-21). |
| `TEXT` | `"` … `"` on one line | escapes in §2.3. |
| `BLOCK_TEXT` | lines of a `plan: \|` block scalar | §3.2. |
| punctuation | `( ) [ ] < > , : . = - + * / ? \| ->` and `== != <= >=` | longest match first. |
| `NEWLINE` `INDENT` `DEDENT` | produced by the layout pass | §3. |

**R-SYN-03** A `NUMBER` literal is converted exactly to a `Number` (D-36). A literal outside its range, or with more
than 28 fractional digits, is `VL0101` ("this number is too big" / "this number has too many decimal places").

**R-SYN-04** Comments start with `#` and run to end of line — except inside a `TEXT` literal or a `BLOCK_TEXT` plan,
where `#` is ordinary text.

### 2.2 Keywords and reserved words

| Kind | Words |
|---|---|
| Keywords (v0.1) | `language type goal call plan check examples budget and or not if then every some in has is empty nothing true false result` |
| Reserved (D-24) | `pure effects when choose otherwise import module fallback retry optional assume` |
| Built-in type names | `Number Text Boolean Nothing List` — ordinary names resolved by sema, not keywords |

**R-SYN-05** Using a reserved word as a name, or as a block/modifier, is `VL0104 ReservedWord` with the message "`when`
is coming in a later Velme version". Using a keyword as a name is `VL0101`. `result` as a parameter, field or quantifier
variable is `VL0101` (D-4): unlike a truly reserved word, `result` is already meaningful in v0.1 (D-62), so misusing it
is an ordinary syntax error, not "coming in a later version" — `VL0104` stays reserved for the D-24 words.

**R-SYN-06** Naming conventions — `PascalCase` for types and goals, `snake_case` for fields, parameters and bindings —
are lint warnings (`VL0107 LintWarning`, D-69), never errors (P-3 applies to meaning, not style).

### 2.3 Text literals

**R-SYN-07** Escapes: `\"` `\\` `\n` `\t` `\u{H…}` (1–6 hex digits, a Unicode scalar value). Any other escape, or a
surrogate code point, is `VL0101`. A line end before the closing quote is `VL0105 UnterminatedText`.

## 3. Layout

**R-SYN-08** Indentation uses spaces only. A tab anywhere in leading whitespace is `VL0103 TabIndentation` (D-19). This
governs the layout pass's own indentation (§3), not the content of a `BLOCK_TEXT` plan (§3.2): a tab appearing inside
the text a `plan: |` block carries is ordinary text and is preserved, never `VL0103` (R-SYN-12).

**R-SYN-09** The layout pass keeps a stack of indentation widths. A line indented deeper than the top emits `INDENT`
and pushes; a shallower line emits one `DEDENT` per popped width and must land exactly on a width in the stack,
otherwise `VL0102 InconsistentIndentation`; a line that lands between two widths stays in the inner block (D-76). Any
consistent width is accepted; 4 is recommended and used by the formatter.

**R-SYN-10** Blank lines and comment-only lines produce no tokens. Inside `( )` and `[ ]` newlines and indentation are
ignored (implicit line joining), so long parameter lists and literals may span lines.

**R-SYN-11** `INDENT` is legal only after a line ending in `:` that opens a block (`type`, `goal`, `call`, `check`,
`examples`). An unexpected indent is `VL0102`.

### 3.1 Inline plan

`plan: "Find the player with the highest jump."` — one `TEXT` token.

### 3.2 Block-scalar plan

**R-SYN-12** `plan: |` followed by a newline starts a block scalar. Its content is every following line indented
deeper than the column of the `plan` keyword, including blank lines between them; it ends at the first non-blank line
indented at or below that column. The first content line's indentation is the content column: a later line indented
less than it, but deeper than `plan`, is `VL0102` (D-73). The lexer emits the content as one `BLOCK_TEXT` token,
normalized per D-21 (LF, trailing spaces and tabs trimmed, the content column removed). No escapes or comments are processed inside it. Trailing blank
lines, tabs, a comment after `|` and an empty block follow D-67.

**R-SYN-13** An empty inline or block plan is `VL0307 GoalHasNoBody` unless the goal is wired (D-4,
[12-goals-calls](12-goals-calls.md) §2).

**R-SYN-22** A Unicode bidi control character (U+061C, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069) inside a `plan`
(inline or block), a `TEXT` literal or a comment is a lint warning, using the naming-convention mechanism of R-SYN-06 (`VL0107`, D-69): these
characters can make displayed and lexed order differ ("Trojan Source"), which matters most in text an LLM reads (D-43,
[compiler/22](../compiler/22-spellbook-synthesis.md) R-SYNTH-22).

## 4. Grammar (EBNF)

Terminals are quoted or upper-case tokens from §2. `{ x }` = zero or more, `[ x ]` = optional. Layout tokens come from §3.

```text
program        = [ header ] { declaration } EOF ;
header         = "language" ":" NAME "/" VERSION NEWLINE ;         (* NAME must be "velme" *)
declaration    = type_decl | goal_decl ;

(* ---- types ---- *)
type_decl      = "type" NAME ":" NEWLINE INDENT field_decl { field_decl } DEDENT ;
field_decl     = NAME ":" type_expr NEWLINE ;
type_expr      = base_type [ "?" ] ;
base_type      = "List" "<" type_expr ">" | NAME ;

(* ---- goals ---- *)
goal_decl      = "goal" NAME "(" [ param { "," param } [ "," ] ] ")" "->" type_expr ":"
                 NEWLINE INDENT goal_body DEDENT ;
param          = NAME ":" type_expr ;
goal_body      = [ budget_line ] [ call_block ] [ plan_block ] [ check_block ] [ examples_block ] ;

budget_line    = "budget" budget_item { budget_item } NEWLINE ;
budget_item    = NAME "=" NUMBER [ UNIT ] ;

call_block     = "call" ":" NEWLINE INDENT binding { binding } DEDENT ;
binding        = ( NAME | "result" ) "=" NAME "(" [ call_arg { "," call_arg } [ "," ] ] ")" NEWLINE ;
call_arg       = path | literal ;
path           = NAME { "." NAME } ;

plan_block     = "plan" ":" ( TEXT NEWLINE | "|" NEWLINE BLOCK_TEXT ) ;

check_block    = "check" ":" NEWLINE INDENT check_item { check_item } DEDENT ;
check_item     = "-" expr NEWLINE ;

examples_block = "examples" ":" NEWLINE INDENT example_item { example_item } DEDENT ;
example_item   = "-" NAME "(" [ literal { "," literal } [ "," ] ] ")" "==" literal NEWLINE ;

(* ---- literals (examples, call arguments) ---- *)
literal        = [ "-" ] NUMBER | TEXT | "true" | "false" | "nothing"
               | "[" [ literal { "," literal } [ "," ] ] "]"
               | NAME "(" [ NAME ":" literal { "," NAME ":" literal } [ "," ] ] ")" ;

(* ---- expressions (checks) ---- *)
expr           = if_expr | quant_expr | or_expr ;
if_expr        = "if" or_expr "then" expr ;
quant_expr     = ( "every" | "some" ) NAME "in" postfix "has" expr ;
or_expr        = and_expr { "or" and_expr } ;
and_expr       = not_expr { "and" not_expr } ;
not_expr       = "not" not_expr | cmp_expr ;
cmp_expr       = add_expr [ cmp_op add_expr | "is" [ "not" ] "empty" ] ;
cmp_op         = "==" | "!=" | "<" | "<=" | ">" | ">=" ;
add_expr       = mul_expr { ( "+" | "-" ) mul_expr } ;
mul_expr       = unary { ( "*" | "/" ) unary } ;
unary          = "-" unary | postfix ;
postfix        = primary { "." NAME } ;
primary        = NUMBER | TEXT | "true" | "false" | "nothing" | "result"
               | NAME [ "(" [ arg { "," arg } [ "," ] ] ")" ]
               | "[" [ expr { "," expr } [ "," ] ] "]"
               | "(" expr ")" ;
arg            = [ NAME ":" ] expr ;          (* named ⇒ record literal; sema decides, §4.2 *)
```

**R-SYN-14** Goal body blocks appear at most once each and in the order `budget`, `call`, `plan`, `check`, `examples`.
A block out of order or repeated is `VL0101` with a hint naming the expected order.

**R-SYN-15** `T??` is `VL0101`. A header naming another language or a version this compiler doesn't support is
`VL0106 UnsupportedLanguageVersion`, and so is a version written in another form, such as `velme/1`. Without a header the compiler's current language version applies and is recorded in
the artifact (INV-8).

**R-SYN-21** The header's `VERSION` is compared to the compiler's supported versions as text, never decoded as a
`Number`: `velme/0.10` and `velme/0.1` are different versions even though `0.10 == 0.1` numerically. A version the
compiler doesn't support is `VL0106` (R-SYN-15).

### 4.1 Precedence (lowest → highest)

| Level | Forms | Associativity |
|---|---|---|
| 1 | `if A then B`, `every x in xs has P`, `some x in xs has P` | right side extends as far as possible |
| 2 | `or` | left |
| 3 | `and` | left |
| 4 | `not` | prefix |
| 5 | `== != < <= > >=`, `is empty`, `is not empty` | non-associative (`a < b < c` is `VL0101`) |
| 6 | `+ -` | left |
| 7 | `* /` | left |
| 8 | unary `-` | prefix |
| 9 | `.field`, call `f(…)` | postfix |

An `if` or quantifier inside an `and`/`or` operand needs parentheses.

### 4.2 Call vs record literal

**R-SYN-16** `Name(…)` in an expression is parsed uniformly; sema resolves it: a type name ⇒ record literal (all args
must be named), a built-in ⇒ built-in call (all args positional). Mixing named and positional args is `VL0101`.
Goals are never called from checks ([13-check-dsl](13-check-dsl.md) R-CHK-03).

## 5. Error recovery

**R-SYN-17** The parser recovers at `NEWLINE` / `DEDENT` boundaries and at the next `type`/`goal` keyword, and reports
every syntax error in the file (default cap 20, then "…and N more"), one per root cause (D-76). A declaration that
failed to parse, or holds any other error (a lexer error, a reserved word), is excluded from sema; diagnostics that would only follow from it (e.g. `VL0301` for a goal whose declaration failed) are suppressed.

**R-SYN-18** Every syntax diagnostic has a code, a primary span, a learner-friendly message and, where one exists, a
hint ("did you mean `check:`?"). Messages never mention tokens by internal name (`INDENT`, `NAME`).

**R-SYN-19** Parsing is deterministic and total: any input terminates with an AST or diagnostics, never a panic (fuzzed,
[51-testing-quality](../delivery/51-testing-quality.md)). Nesting on one line is capped so that parsing can't overflow
the stack (D-71).

## 6. Golden corpus

**R-SYN-20** Every syntax rule has at least one accepting file in `tests/golden/parser/accept/` and one rejecting file
in `tests/golden/parser/reject/`. Each file's `insta` snapshot (`delivery/51` R-QA-03) is its AST as JSON, with byte
spans on every node, or its diagnostics as JSON (D-70). A later Tree-sitter grammar must give the same accept/reject
outcome on the whole corpus (D-25).

## 7. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-SYN-01 | The complete program in [12-goals-calls](12-goals-calls.md) §8.1 parses with no diagnostics and its AST matches the golden file. |
| AC-SYN-02 | A tab in leading whitespace yields `VL0103` at that line; the rest of the file is still parsed. |
| AC-SYN-03 | A dedent to a width not on the stack yields `VL0102`. |
| AC-SYN-04 | `plan: \|` content keeps internal blank lines and `#` characters, strips common indentation, and ends at the first line at or below the `plan` column. |
| AC-SYN-05 | `goal A(when: Number) -> Number:` yields `VL0104` whose message says the word is coming in a later version. |
| AC-SYN-06 | `"abc` followed by a newline yields `VL0105`; `"\q"` yields `VL0101`. |
| AC-SYN-07 | A file with three unrelated syntax errors reports all three, in source order, and no cascaded `VL0301`. |
| AC-SYN-08 | `language: velme/0.2` on a 0.1 compiler yields `VL0106`; a file without a header compiles as 0.1. |
| AC-SYN-09 | `a < b < c` in a check yields `VL0101`; `not a and b` parses as `(not a) and b`; `a or b and c` as `a or (b and c)`. |
| AC-SYN-10 | `if a then b or c` parses as `if a then (b or c)`; `every x in xs has x > 0 and x < 9` quantifies the whole conjunction. |
| AC-SYN-11 | A parameter list split across lines inside `( )` parses as if written on one line. |
| AC-SYN-12 | `check:` placed before `plan:` yields `VL0101` with a hint listing the block order. |
| AC-SYN-13 | The parser terminates without panic on every input in the fuzz corpus. |
| AC-SYN-14 | A file with a UTF-8 BOM followed by valid source compiles with no diagnostics; a byte sequence that isn't valid UTF-8 yields `VL0901`. |
| AC-SYN-15 | A file containing a lone `\r` not followed by `\n` yields `VL0101`; a file using only `\r\n` line endings parses identically to the same source with `\n`. |
| AC-SYN-16 | A `NUMBER` literal with 29 fractional digits yields `VL0101` ("too many decimal places"); one whose coefficient exceeds `2^96` yields `VL0101` ("too big"). |
| AC-SYN-17 | `Player??` yields `VL0101`; on a compiler that supports only `0.1`, `language: velme/0.10` yields `VL0106` while `language: velme/0.1` compiles (R-SYN-21). |
| AC-SYN-18 | `Player(name: "A", 3)` (mixed named and positional arguments) yields `VL0101`; `sum(1, 2)` in a check parses as a built-in call and `Player(name: "A", jump_height: 3, score: 1)` as a record literal. |
