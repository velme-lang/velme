# RFC 0002: Effects, loops and recursion

- **Status:** draft, revised with the owner's answers of 2026-11-06. It records the owner decisions of 2026-11-03 as D-172; the points the revision opened are batched as new questions to the owner. It must reach **accepted** before merge (R-REL-11). P4a changes the specification only and does not grant that approval.
- **Spec ids touched:**
  - **Invariants, principles, SPEC sections:** INV-3, INV-4, INV-5, INV-6 (new text); P-5; SPEC §4, §6.
  - **Syntax and goals:** R-SYN-05, R-SYN-11, R-SYN-14, R-SYN-15, §2.2, §4 (new R-SYN-25–28); R-GOAL-03, R-GOAL-13, R-GOAL-14, R-GOAL-16, R-GOAL-17 (new R-GOAL-24–31). R-GOAL-11 is cited but unchanged.
  - **Types, checks, builtins:** R-TYP-34 (new); R-CHK-16 (new); R-BLT-02, R-BLT-03, R-BLT-05, R-BLT-09 (new R-BLT-13–17).
  - **IR:** R-IR-01, -02, -06, -09, -10, -12–17, -22, -26 (new R-IR-27–32).
  - **Synthesis:** R-SYNTH-08, -14, -17, -22, -38, -50–59 (new R-SYNTH-74–80).
  - **Runtime and sandbox:** R-RUN-05, -06, -09, -12–14, -17, -19, -24, -25 (new R-RUN-29–39); R-SBX-02, -03, -05, -08, -09, -10, -11, -17, -19, §5, §6, §8; R-SBX-04 and -06 cited.
  - **Artifacts:** R-ART-23, -24, -26, -27, -28 (new R-ART-29–30).
  - **Security:** R-SEC-02–04, R-SEC-08; T-15 (amended), T-16–T-21 (new R-SEC-14–18).
  - **CLI and errors:** R-CLI-02, -05, -10, -11, -16, -27 (new R-CLI-31–35); `reference/90` codes, `VL0902` cited.
  - **Decisions and questions:** D-8, D-22, D-24, D-27, D-29, D-30, D-51, D-84, D-87, D-90, D-112, D-113, D-123 amended or cited; D-53, D-54, D-88, D-114, D-134 cited; D-172–186 new; Q-14 closed; Q-18 and Q-26 cited; Q-36–40 new.
- **Invalidates locks:** yes, in one coordinated `0.3.0-alpha.1` release.

## Summary

Velme 0.3 lets a goal use **effects** that it declares (`effects fetch, write`) and that the project grants in `velme.toml`.

- **First capabilities:** `clock`, `random`, `read`, `fetch` and `write`.
- **Reading effects** are IR nodes that the validator checks against the goal's signature. `fetch` returns every HTTP status as a value (`HttpResponse`).
- **Files are written by returning `OutputFile` values.** Velme saves them only after the whole run succeeds.
- **Effect limits and options are project config** (`[effects.limits]`, `[effects.options]`), each with a default and a fixed ceiling. Config may widen reach, never change meaning, and stays out of goal keys.
- **Build and test never perform an effect.** Examples state the effect results with `given:`, and verification generates the rest.

A `call` block gains:

- `name = for x in list: Child(…)`, which calls the child once per item;
- `when`/`otherwise` and `choose`, which pick calls from a fixed set by a check-language condition.

Inside a goal, the model may write **local recursive functions** (`letfn`/`apply`). These are fuel-metered and limited to 256 frames deep, and they never call goals.

Every new node runs on both backends: the WASM emitter gains local functions and JSON as emitted code, and effects as new host imports that call the interpreter's own handler.

The release accepts only `velme/0.3`, IR `0.3` and builtins `0.3`. A goal with no `effects` line behaves exactly as in 0.2.

## Motivation

Velme 0.2 programs can't read a file, call an API, use the date, fan a goal out over a list, or pick a goal by a rule. Iteration inside a goal stops at `map`/`reduce` (R-GOAL-17, P-5).

The 2026-11-03 decision (D-172) aims Velme at non-coders and professionals equally, for all kinds of programming. That needs input from the world, output to it, data-dependent composition and real algorithms. It must keep the properties that make Velme trustworthy:

- the model never gains power (INV-1);
- checks stay deterministic (INV-2);
- every run is bounded (INV-5);
- a beginner's pure goal behaves exactly as before (P-2).

## Design

### 1. Effects

**Capabilities.** P4 has five capabilities, each with a closed set of operations. The operation catalog is part of `builtins_version` (R-BLT-09).

| Capability | Operation (IR `effect` node / notation call) | Result | Notes |
|---|---|---|---|
| `clock` | `now_ms()` | `Number` | Milliseconds since the Unix epoch (UTC), read **once per run**; every call in the run returns that value. |
| `random` | `fresh_seed()` | `Number` | A whole number in [0, 2⁶³), derived from the run seed (see below). Use it with `random(seed, index)` (D-22). |
| `read` | `read_text(path)` | `Text?` | `nothing` if the file doesn't exist. UTF-8 only. At most `read_max_bytes`. |
| `read` | `list_files(dir)` | `List<Text>` | Files directly in `dir`, as paths in the grant's form, in byte order. At most `max_list_size` items. |
| `fetch` | `fetch(url)` | `HttpResponse` | HTTPS GET. Every HTTP status is a value (§1.7). Body at most `fetch_max_bytes`. |
| `fetch` | `fetch_text(url)` | `Text?` | HTTPS GET. The body on 2xx, `nothing` on 404 or 410, `VL0608` on any other status after retries. |
| `write` | — | — | No operation. A goal declaring `write` saves the `OutputFile` values in its output (§1.4). |

How `fresh_seed()` is derived: take BLAKE3 of the canonical `{"n": k, "path": P, "run_seed": S}`, then take its first 8 bytes as a u64 shifted right by 1. Here:

- `S` is the run seed, from `--seed N` or the operating system's RNG, and recorded;
- `P` is the invocation path: binding names and loop indices from the root;
- `k` is the call's ordinal within its invocation, counting every effect operation.

So concurrent goals get distinct seeds, and the result never depends on scheduling (INV-3).

Deferred to later RFCs, as new open questions:

- secrets, environment variables and authenticated requests (Q-36);
- HTTP methods other than GET, stdout, processes and a live clock (Q-37).

#### 1.1 Declaration (R-SYN-25, R-GOAL-24–26)

The `effects` line is the first line of a goal's body, before `budget`. R-SYN-14's block order becomes `effects`, `budget`, `call`, `plan`, `check`, `examples`.

```velme
goal FetchWeather(city: Text) -> Text?:
    effects fetch
    plan: "Fetch https://api.example.com/v1/weather?city= followed by the city."
```

- `effects` becomes a keyword. The capability names stay contextual, so they remain legal names elsewhere.
- An unknown or repeated capability name is `VL0310`.
- Declaration is **transitive**: a goal's set must contain every effect its `call` children declare. Otherwise `VL0309`, with help naming the child and the missing word. So a goal with no `effects` line is pure in its whole subtree; R-GOAL-03 is restated that way.
- `pure` stays reserved, with no meaning.
- The sorted effect set is a signature member, `effects`, omitted when empty (the D-165 pattern). It therefore enters the goal's `contract_key` and its callers' keys.

#### 1.2 Grants, limits and options (R-SEC-14–15, R-CLI-31–32)

```toml
[effects]
clock = true
random = true
read = ["data", "/srv/shared/rates"]          # project directories; an absolute entry needs approval
fetch = ["api.example.com", "*.cdn.example.com"]
write = ["out"]

[effects.limits]                              # each key has a default and a fixed ceiling (table below)
fetch_max_ops = 500

[effects.options]
fetch_redirects = "same-host"

[effects.record]                              # what `--record` keeps of each result: full | digest | none (§5.2)
read = "digest"
fetch = "digest"
```

**The rule for all effect config:** config may widen reach (more paths, hosts, operations, bytes or time), never change meaning. What an operation returns for a given world is fixed by the language. None of `[effects]`, `[effects.limits]`, `[effects.options]` or `[effects.record]` enters any goal's keys. A bad value in any of them (wrong type, unknown key, unknown option, a value above its ceiling or below 1) is the existing config error `VL0902`.

**Granted paths:**

- They use `/`, are non-empty, and have no `.`/`..` component.
- A relative entry is under the project root, and is not `.velme` or anything inside it.
- An absolute entry (outside the project) is allowed, but it makes that capability need the user's approval, `read` included, and `velme allow` shows it prominently.

**Operation paths:**

- They are project-relative, or absolute inside an absolute grant, at most 1 024 bytes, and inside a granted directory.
- A `..` component is always refused.
- Symbolic links are not followed unless `read_follow_symlinks` or `write_follow_symlinks` is `true`; then the link's target must resolve inside a grant of the same capability.
- They are never `velme.toml`, `velme.lock`, `.velme/**` or a `*.velme` file when writing.

**Hosts:**

- An entry is an exact host name, compared as text, or a wildcard `*.example.com`, which matches any host ending in `.example.com` with at least one more label, and not `example.com` itself. Approval spells each wildcard out.
- Plain `http` is allowed only to a loopback host whose port is granted (`"localhost:8080"`).
- An address that resolves to a link-local range or a cloud metadata address (for example 169.254.169.254) is always refused, on every redirect hop. A literal address in those ranges and a known metadata host name (such as `metadata.google.internal`) are refused before any lookup.
- With `fetch_proxy = "env"`, Velme still resolves each host, every redirect hop included, itself and refuses link-local or metadata answers before sending the request through the proxy.
- Requests send only a fixed `User-Agent`. Redirects and proxies follow `[effects.options]`.

**`[effects.limits]`** (R-RUN-31). A value is a whole number or a string with a unit:

- a bare number means bytes for a `*_bytes` key and seconds for `fetch_timeout` and `fetch_max_wait`;
- a string must carry a unit: `B`, `KiB`, `MiB` or `GiB` for sizes, `ms`, `s` or `min` for times, after a whole number and an optional space (`"16 MiB"`, `"500ms"`);
- a string with no unit or an unknown one (`"16MB"`) is `VL0902`, with help listing the units;
- diagnostics and `velme allow` show values in the largest unit that keeps them whole (`16 MiB`, `90 s`).

| Key | Default | Ceiling | What it bounds |
|---|---|---|---|
| `read_max_bytes` | 1 MiB | 64 MiB | one `read_text` result |
| `read_max_ops` | 256 | 10 000 | `read_text` and `list_files` calls per run |
| `fetch_max_bytes` | 1 MiB | 64 MiB | one response body, after decompression |
| `fetch_max_ops` | 256 | 10 000 | `fetch` and `fetch_text` calls per run; a retried request counts once |
| `fetch_timeout` | 10 s | 120 s | one attempt, redirects included |
| `fetch_max_wait` | 120 s | 1 h | all fetch time in a run: every attempt and every backoff |
| `fetch_retries` | 2 | 5 | extra attempts per request (§1.7) |
| `write_max_files` | 1 000 | 100 000 | staged files per run |
| `write_max_bytes` | 16 MiB | 1 GiB | staged bytes per run |

An effect result is still charged fuel and memory (§1.3), so a goal's own budget may be the tighter limit.

**`[effects.options]`:**

| Key | Default | Values |
|---|---|---|
| `read_follow_symlinks` | `false` | `true` follows a link whose target resolves inside a `read` grant |
| `write_follow_symlinks` | `false` | `true` follows a link whose target resolves inside a `write` grant |
| `fetch_redirects` | `"same-host"` | `"none"`, `"same-host"`, `"granted-hosts"`; every hop is checked against the grant and the floor |
| `fetch_max_redirects` | 5 | at most 10 |
| `fetch_proxy` | `"none"` | `"none"`, `"env"` (honour `HTTPS_PROXY`/`NO_PROXY`) |

**The fixed floor**, which no config changes:

- protected files are never writable;
- link-local and metadata addresses are always refused;
- `..` in an operation's argument is always refused;
- plain `http` goes only to a granted loopback port;
- paths are at most 1 024 bytes and URLs at most 2 048 bytes;
- the semantics are fixed: a missing file is `nothing`, text is UTF-8, `list_files` is sorted and non-recursive.

**Before the entry goal runs,** the runtime compares its transitive effect set with the effective grant. The effective grant is the project grant, narrowed by the user's approval. A missing capability is `VL0802`, and no goal runs.

**`fetch` and `write` also need the user's approval,** for the same reason as D-50: a cloned project must not grant itself network access or write access. `read` needs it too when its grant has an absolute entry.

- `velme allow` records the project root and the BLAKE3 of its canonical `[effects]` table, with `[effects.limits]` and `[effects.options]` and without `[effects.record]`, in the user-level `approvals.toml`. It lists hosts, directories, wildcards and outside entries, and every limit or option that differs from its default.
- `--allow-effects` approves the grant for one run only.
- A changed grant, limit or option needs approving again.
- `clock`, `random` and project-only `read` need no approval.

**At run time:**

- A path or host outside the grant, or refused by the floor, is `VL0803`. This is deterministic, since it depends only on the arguments.
- Under `--offline`, a run whose subtree declares `fetch` is `VL0802`.

#### 1.3 IR and validation (R-IR-27–28)

- The envelope gains a compiler-owned `effects` array (`[]` when pure), filled from HIR like `inputs` (D-103).
- An effect call is `{"kind":"effect","op":"fetch_text","args":[E]}`. It is not a `builtin`, so checks can never call it (R-CHK-03).
- **Validation stages:**
  - stage 3: an unknown `op` is `VL0402`, like an unknown builtin (D-63);
  - stage 4: argument and result types are checked;
  - stage 5: an `op` whose capability isn't in `effects` is `VL0801` (R-IR-15 now has a node to guard);
  - stage 6: `effects` must equal HIR.
- Effects run in evaluation order (R-RUN-02).
- **Fuel:** 1 before the operation is performed, then ⌈result bytes/64⌉ plus 1 per list item or header once it returns. The result is then charged memory (D-89). A charge that fails after the operation is performed still records the effect event.
- Every artifact manifest gains `effects`, omitted when empty (R-ART-29), and keeps `velme-artifact/1`.

#### 1.4 Writing files (R-TYP-34, R-RUN-33–34)

`OutputFile` is a prelude record type, always visible: `OutputFile(path: Text, text: Text)`.

- Any goal may build `OutputFile` values, and checks and examples see them like any record.
- When a goal that declares `write` succeeds (output membership and checks pass), its output is walked in type order, like R-RUN-26. Every `OutputFile` found is **staged**.
- Staged files are committed only after the whole top-level run succeeds: each file is written atomically (temporary file, then rename), in staging order.
- Identical duplicates collapse. Two different texts for one path is `VL0612`, and nothing is written.
- Limits: `write_max_files` and `write_max_bytes` per run (`VL0609`). Each text is already capped by `max_output_bytes`.
- A run that fails writes nothing. Reads never see this run's writes, so effectful goals stay free to run concurrently.
- Text only, and no append mode.
- Human output lists `Saved out/paris.txt`. `--json` adds `results[].saved` (additive, R-CLI-27).

#### 1.5 What the model sees (R-SYNTH-74)

The prompt gains an **Effects** section only for a goal that declares effects, about 25 tokens per operation. It lists that goal's operations with their signatures, plus two rule lines:

- "tests give made-up results of these types: handle `nothing`, any text and any HTTP status";
- "save a file by returning `OutputFile`".

The `HttpResponse` and `HttpHeader` definitions appear only in the prompt of a goal that declares `fetch` or whose signature uses them. Undeclared operations are never listed. In the notation, operations are called by name like builtins (R-SYNTH-52).

Effect results never reach a provider:

- build and test perform none;
- `given` stubs are author text, fenced like examples (R-SYNTH-22);
- R-SEC-08 gains: "values produced by effects are never sent to a provider". Coach (Future) must treat run records as untrusted.

#### 1.6 JSON (R-BLT-13–16)

Two additions make `read` and `fetch` usable:

- **`decode_json(text, T) -> T?`.** It is the IR node `{"kind":"decode","text":E,"as":T}`.
  - It follows D-23, except that **unknown object members are ignored**, because API responses carry extra fields.
  - A type mismatch, a missing required field, a duplicate of a declared field, malformed JSON or an inexact number gives `nothing`. Duplicates among ignored members are not checked.
  - `T` is a body annotation type (no Domain, R-IR-25), is not optional at the top level, and must be reachable under D-84. So decoding into a record not in the signature belongs in a pure child goal.
  - Domain-typed fields come back as base values; output membership (R-RUN-26) checks them, as for any record a body builds.
  - Skipped members are walked iteratively (R-RUN-25).
  - Fuel: 1 + ⌈bytes/64⌉ + 1 per JSON value.
- **`encode_json(v) -> Text`.** It produces canonical JSON (R-IR-21). Fuel: 1 + ⌈output bytes/64⌉ + 1 per value.
- Both are callable from checks and from IR.
- `split(text, sep) -> List<Text>` and `to_number(text) -> Number?` join them, so CSV-like text is usable without a parser.

#### 1.7 HTTP responses and retries (R-TYP-34, R-BLT-17, R-RUN-32)

`HttpResponse` and `HttpHeader` are prelude record types, always visible:

- `HttpResponse(status: Number, headers: List<HttpHeader>, body: Text)`;
- `HttpHeader(name: Text, value: Text)`, with `name` lowercased, in received order.

A user type named `OutputFile`, `HttpResponse` or `HttpHeader` is `VL0203`, with a rename hint.

- **`header(r, name) -> Text?`** is a pure builtin: the value of the first header whose name equals `name` lowercased, or `nothing`.
- **Headers** are capped at 64 and 16 KiB in total; `set-cookie` is dropped. A response past either cap is `VL0608`.
- **`fetch`** returns the final response, whatever its status. A 3xx that the redirect options don't follow is returned as it is.
- **`fetch_text`** returns the body on 2xx and `nothing` on 404 or 410. Any other final status is `VL0608`.
- **`VL0608`** otherwise means a transport failure that survived the retries: DNS, TLS, connection, timeout, too many redirects, a body or headers past their cap, or a body that isn't UTF-8.

**Retries** (`fetch_retries`, default 2):

- Only timeouts, connection resets, DNS failures and the statuses 429, 502, 503 and 504 are retried. Other 4xx statuses never are. GET only; a later POST needs its own opt-in.
- Backoff is exponential and deterministic: 0.5 s, 1 s, 2 s and so on, with no jitter. A `Retry-After` header is honoured up to `fetch_timeout`.
- Every attempt and every wait counts toward `fetch_max_wait`. The request counts once toward `fetch_max_ops`.
- Only the final result is traced and recorded, with the retry count.

### 2. Conditional calls (R-SYN-26, R-GOAL-27–28, R-IR-29, R-RUN-29)

```velme
    call:
        total = Subtotal(order.items)
        when order.member:
            price = MemberPrice(total)
        otherwise:
            price = ListPrice(total)
        choose:
            when total > 500:
                gift = BigGift(order)
            when total > 100:
                gift = SmallGift(order)
        result = Receipt(price, gift)
```

**Conditions:**

- A condition is a check expression (R-CHK-16), typed `Boolean` (`VL0204`).
- Its scope is the inputs and the earlier bindings, with narrowing.
- It is lowered by R-CHK-11, stored in the compiler-owned `calls`, and evaluated by `velme-runtime` through `velme-check` on the parent's budget, like call arguments (D-91).
- A value error in a condition fails the parent with its own diagnostic, and the arms are `skipped`.

**Arms:**

- `when` takes an optional `otherwise`. `choose` runs the first `when` arm that is true, in source order, and its `otherwise` is optional.
- Every arm of one block binds the same names (`VL0311`).
- A name's type is the arms' join (D-163). Without `otherwise` it is `T?`, so passing it where `T` is expected is `VL0204`, with help suggesting `otherwise:`.
- An arm holds bindings or `for` bindings; blocks don't nest in P4.
- `result` may be bound in every arm of the final block when that block has an `otherwise`, which makes a wired conditional goal.

**Waves and traces:**

- A block's wave is 1 plus the maximum wave of everything it references. The taken arm runs in sub-waves, and bindings in other arms appear as `not_taken`.
- The trace gains a `gate` event (condition text, value, arm taken).
- `velme explain` renders "When …: / Otherwise:".
- The static call count takes the most expensive arm.

### 3. Loops and run limits (R-SYN-27, R-GOAL-29–30, R-IR-30, R-RUN-30–32)

```velme
goal Reports(cities: List<Text>) -> List<OutputFile>:
    effects fetch, write
    call:
        result = for c in cities: CityReport(c)
    plan: "One report per city."
```

**The loop:**

- `for` iterates a path of type `List<T>`, binding `x` only for that call's arguments (`VL0306` if the name is taken).
- The binding's type is `List<Child output>`, in list order, and `[]` makes no call.
- There is one call per item, and `for` lines don't nest; nesting goes through a child goal.
- Iterations are siblings in one wave: they run concurrently on the pool, and D-9 applies, with the lowest failing index reported.
- Assembling the result list charges the parent 16 bytes plus the items' memory (D-89), and 1 fuel per item.
- The trace's `call` gains `index`, and `explain` renders "For each c in cities: …".

**Run limits.** `max_goal_calls` rises from 128 to **10 000** for the whole run:

- it is still checked statically when a program has no `for`;
- otherwise it is metered at run time;
- the `calls=` budget key still tightens it.

A new **`max_run_fuel` = 5 × 10⁹** (50 s at `FUEL_PER_MS`) sums fuel across the run. It replaces D-51's static product as the run's deterministic bound and stays below the 60 s watchdog. Programs within 0.2's limits (at most 128 × 10⁷ fuel) can never reach it.

**Both run-wide counters give deterministic verdicts despite concurrency.** The reason:

- A partial run starts only invocations that the complete run also starts. Wave starts, loop lengths and arm choices depend only on values, given the same effect results.
- Each counter only grows.
- So "the counter passed its cap" holds exactly when the complete run needs more than the cap.

When a cap is crossed, the run stops starting invocations and lets the running ones finish. It then reports `VL0605` (calls) or `VL0611` (fuel), which outranks every other failure. The trace keeps only the root event, marked `truncated`, since which calls happened to run depends on timing.

The run counters of `[effects.limits]` (`read_max_ops`, `fetch_max_ops`, `write_max_files`, `write_max_bytes`) use the same rule and report `VL0609`, naming the capability and its key. `max_call_depth` stays static.

**The watchdog** (`VL0603`) excludes time spent waiting on effects. Fetch time is bounded by `fetch_max_wait`: when it runs out, the current attempt stops and the request fails with `VL0609` naming `fetch_max_wait`. That failure depends on timing, so it is recorded like an effect failure and is not a monotone verdict. Reads open regular files only, so their wait is bounded by `read_max_ops` × `read_max_bytes`; a FIFO, device or socket is `VL0608`.

### 4. Recursion inside a goal (R-IR-31–32, R-RUN-35–36, R-SYNTH-75–77)

```json
{"kind":"letfn","fns":[{"name":"ways","params":[["n",{"t":"Number"}]],"output":{"t":"Number"},
  "body":{"kind":"if","cond":…,"then":…,"else":{"kind":"binary","op":"add",
    "left":{"kind":"apply","fn":"ways","args":[…]},"right":{"kind":"apply","fn":"ways","args":[…]}}}}],
 "body":{"kind":"apply","fn":"ways","args":[{"kind":"input","name":"steps"}]}}
```

**What the validator accepts:**

- `letfn` appears only at the root of a body. Its functions may call themselves and each other, and see the inputs, call bindings and their own parameters.
- Parameter and output types are body annotation types.
- Function names may not reuse an input, binding, builtin or operation name.
- A function body nests at most 64 levels.
- `apply` targets only a function of the group. Lambdas stay non-values (R-IR-04).
- Goals are never reachable from IR, so R-GOAL-11 and the acyclic goal graph are unchanged.

**How it runs:**

- `apply` charges its entry fuel (`VL0601`), evaluates its arguments left to right, then checks the depth.
- Depth above `max_recursion_depth` = 256 frames per invocation is `VL0610`, and the body does not run.
- Frames are not charged memory.
- Tail calls aren't special.
- Termination comes from fuel.
- R-RUN-25 gains the bound 256 × 64 levels on top of its current figure. That fits the CLI's 64 MiB stack, and a test at the deepest case keeps it so. The WASM figure is §6.1's.

#### 4.1 The scratch stack (R-RUN-39)

A local function called inside a `map` runs its own collection nodes while the caller's is still open. So the working space of collection nodes becomes a last-in, first-out **scratch stack** on both engines, for every collection node (`map`, `filter`, `find`, `reduce`, `all`, `any`, `sort_by`, comprehensions included, since they lower to these), not only under `letfn`.

- **Frames:** a collection node pushes a frame of **64 + 24 × n** bytes, n being the length of the list it visits, when it starts, and pops it when it ends, failing or not.
- **Budget:** the frames open at once may total at most **`SCRATCH_STACK_BYTES` = 16 MiB** per invocation, a shared constant (D-54). One counter covers all evaluation in `velme-interp`: goal bodies, call arguments and conditions, checks and example expressions. The stack is **not charged as memory**: it has its own budget, so `max_memory` and every 0.2 memory figure are unchanged.
- **Failure:** a push past the budget is `VL0610`, with the message "nested too deeply", and the node visits nothing.
- **Order** (D-88's steps): the node's entry fuel (`VL0601`), then its `list` argument, then the push (`VL0610`), then step (2), visiting the elements, where `VL0602`, `VL0606` and `VL0604` arise, then the remaining fuel. So the push comes after the node's entry fuel and before any element, size or memory check.
- **Same on both engines:** the interpreter tracks the identical counter. It is deterministic because it depends only on the specified evaluation order: a frame lives exactly from its node's start to its end, and nodes start and end in R-RUN-02's order.
- **Not what D-53 rejected:** D-53 rejected a peak of *live* bytes because it needs liveness tracking, which the interpreter doesn't do and WASM's never-freeing bump allocator can't match. Here the lifetime is lexical: push and pop are fixed points of evaluation, so the counter is a plain sum of open frames, with no liveness analysis, and the same on any engine that follows the evaluation order.
- **0.2 programs never reach it:** collection nodes nest at most `MAX_COLLECTION_NESTING` = 4 inside one another's lambdas (one in another's `list` position has ended before the outer node pushes), so at most 5 frames are open, each at most 64 + 24 × 10 000 = 240 064 bytes: 1 200 320 bytes in all, under 16 MiB. A check line at D-71's 32 levels of nesting stays under too (7.7 MB). Only `letfn` recursion through a lambda can stack more: 16 MiB holds about 69 frames over 10 000-item lists, or 256 recursion levels over lists of about 2 700 items.
- **Why not charge memory:** any charge per call, cumulative under D-53, grows with the calls, not with the nesting. A bill of materials whose every recursive call scans a 10 000-row list would pay it once per call, which is quadratic in practice.

**In the notation (grammar version 2):**

```python
def ways(n: Number) -> Number:
    if n <= 1:
        return 1
    return ways(n - 1) + ways(n - 2)
return ways(steps)
```

- `def` blocks lead the body and need annotations: `Number`, `Text`, `Boolean`, `list[T]`, `T | None`, or record names.
- An unannotated `def` is `notation-15`. A `def` that isn't at the top is `notation-4`, with a new hint.
- `while` and `for` statements stay `notation-4`.
- The prompt's fixed prefix gains about 180 bytes: one form line, one example and "prefer comprehensions; use `def` only for recursion".

### 5. Verification, recording and replay

#### 5.1 Effects in build and test (R-SYN-28, R-SYNTH-78–80)

```velme
    examples:
        - FetchWeather("Paris") == "{\"t\": 21}" given:
            fetch_text("https://api.example.com/v1/weather?city=Paris") == "{\"t\": 21}"
```

**`given:` stubs:**

- `given` is contextual and opens a block (R-SYN-11). Each line is `op(literals) == literal`, typed against the catalog (`VL0204`). A `fetch` stub's literal is an `HttpResponse` record.
- A stub naming an effect outside the goal's subtree is `VL0310`.
- Stubs answer by (operation, arguments) for the whole run tree of that example.
- An effect the example performs with no stub is `VL0505`, quoting the exact call so it can be copied. The rule is strict for `now_ms`/`fresh_seed` too (P-3).
- Unused stubs are not an error.

**Generated cases:**

- Generated cases draw each new (operation, arguments) result from a stream seeded by `contract_key`, the case index, the invocation path and the ordinal (R-SYNTH-17), and memoize it within the case.
- The draws are:
  - `now_ms`: a whole number in [0, 4 102 444 800 000];
  - `fresh_seed`: a whole number in [0, 2⁶³);
  - `read_text`/`fetch_text`: `nothing` or a Text from the §7 generator;
  - `list_files`: a §7 list of Text;
  - `fetch`: an `HttpResponse` whose status is drawn uniformly from 200, 201, 204, 301, 302, 304, 400, 401, 403, 404, 410, 429, 500, 502, 503 and 504, with a §7 list of headers (lowercase names) and a §7 Text body.
- `OutputFile`s are never committed.

Consequences:

- Build, test and lock re-verification (R-ART-22) **never perform an effect**, need no grant, and keep INV-7.
- Stubs enter `contract_key` as part of examples.
- R-ART-28's input string for an example with stubs becomes `{"given": […], "inputs": …}`.
- The prompt shows each `given` value up to 1 KiB, then `…`.

#### 5.2 Recording and replay (R-RUN-37–38, R-CLI-33–35)

**The trace** (R-RUN-19, additive) gains `effect` events:

- each holds the invocation path, the ordinal, the operation, its arguments, and either a result or a failure, plus the retry count for a fetch;
- payloads follow `[effects.record]`;
- the human view truncates them as R-RUN-20 does.

**Record policy:**

| Capability | Policy |
|---|---|
| `clock`, `random` | Always recorded in full: one `now_ms` and one `run_seed` per run. |
| `read`, `fetch` | Default `digest`: arguments in full, plus `{"digest":"b3:…","bytes":n}`. A `fetch` digest covers the headers and the body; the status is kept in full. `full` keeps the payload. `none` keeps only digests of the arguments and of the result. |
| `write` | Saved files are results, which the trace already holds. |

The trace always holds inputs and call values in full, so the policy controls only raw effect payloads.

**Recording a run.** `velme run|trace --record RUN.json` writes `velme-run/1`. It contains the format, goal, `execution_id`, versions, inputs, `run_seed`, `now_ms`, the effect events, and the outcome code with a digest of the result.

**Replaying a run.** `velme run --replay RUN.json`:

- re-executes with the recorded results and never writes;
- refuses with `VL0904` on another `execution_id` or format, or when any read/fetch payload isn't `full`;
- with `--live`, re-reads digest-recorded payloads and requires equal digests (`VL0904` names the first that changed).

Divergence under an equal `execution_id` is `VL0607`.

### 6. Backends (INV-3, P-4, D-184)

| Construct | Interpreter | WASM (P4) |
|---|---|---|
| `for`, `when`, `choose`, run counters | scheduler-level in `velme-runtime` | Same. Children run on any backend. |
| `letfn`/`apply` | reference semantics (§4) | emitted functions with a depth counter (§6.1) |
| `decode`, `encode_json`, `split`, `to_number`, `header` | catalog code | emitted code, never imports (§6.2, D-112) |
| `effect` nodes | through an `EffectHandler` trait in `velme-builtins` (shared execution types, D-114) | new host imports that call the same handler (§6.3) |
| `OutputFile`, `HttpResponse`, `HttpHeader` | plain records | plain records |

The real handlers live in `velme-runtime`, reusing the HTTPS client `velme-synth` already uses. The stub, generated and replay handlers implement the same trait. The runtime hands one handler to whichever backend runs a leaf. No crate edge is added (INV-9).

The emitter still declines one shape, with D-123's decline note, and `auto` runs it on the interpreter: a function past `MAX_FN_LOCALS` (§6.1).

#### 6.1 Local functions

- Each function of a group is one WASM function of the module. Parameters and the output cross as slots: a `Number` as two `i64`, anything else as an `i32` address (§3). Inputs are read through `input_ptr`, kept in a module global.
- **Depth:** the module exports a fourth mutable global, `velme_depth_left` (`i32`). The host sets it to `max_recursion_depth` before `velme_run`, like the other limits (D-113: limits are never baked in).
- **Order**, the interpreter's: `apply` takes its entry fuel off `velme_fuel_left` (reason 1, `VL0601`), evaluates the arguments left to right, then decrements `velme_depth_left`. At 0 it sets `velme_reason` to a new reason **5** and traps, which the host maps to `VL0610` with the same `velme-builtins` failure (D-114), so the wording is identical. The function's return increments the global.
- R-SBX-03 lists the fourth global; R-SBX-05 and R-SBX-11 gain reason 5, and a reason outside 0..5 is `VL0607`.
- **Stack:** emitted code recurses only through `apply`, at most `max_recursion_depth` frames deep. The emitter bounds each function to at most `MAX_FN_LOCALS` WASM locals, a named constant of `velme-wasm`; with the 64-level body bound, 256 frames fit the 4 MiB WASM stack (§6), and the emitter declines a function past the constant. A test runs the deepest case on WASM. A stack overflow stays `VL0607`, a bug. This is R-RUN-25's WASM figure.
- **Scratch stack** (§4.1), replacing D-113's fixed frames for every collection node:
  - The region starts at address 0: a fixed base frame of 88 bytes, then `SCRATCH_STACK_BYTES`. Literal data follows the region, and the bump allocator starts after that. Every instance therefore starts with at least 16 MiB of memory; untouched pages are zeroed lazily on Linux and macOS.
  - **Cost check:** P4g measures the per-run cost against D-134's workloads. If the region shows in it, the fallback is `SCRATCH_STACK_BYTES` = 4 MiB; check and example evaluation (worst case about 7.7 MB at D-71's 32 levels) then needs its own allowance on the interpreter. P4g settles this and records the figure.
  - The base frame holds, at offset 0, the address of the top frame; at offset 16, 48 marshalling bytes for `to_text` outside any collection node; and at offset 64 the result slot (24 bytes). It is outside the budget, and the interpreter has no counterpart, which it doesn't need.
  - A frame is a 64-byte header, then 24 bytes per item of the list its node visits. The header holds, at offset 0, the visiting index of R-SBX-19 (0, or 1 + the item's index); at offset 8, the address of the frame below; at offset 16, 48 marshalling bytes.
  - A push is a bump of the top pointer, emitted code checking it against the region's end; past it, `velme_reason` is a new reason **6** and the trap is `VL0610` with the "nested too deeply" message. A pop restores the pointer. Neither touches `velme_memory_left`.
  - After a trap, the host walks the frames from the top through their links, innermost first, to name the items of a failure (D-118). A link outside the region or not below its frame, or a visiting index above `max_list_size`, is `VL0607`. `to_text`'s pointer must be the marshalling bytes of a frame on that chain.
  - A function called inside a lambda pushes its frames above its caller's, so no frame is reused, nothing is copied, and there is no decline.
- **Sandbox text amended:** R-SBX-19 and D-113 (3) describe the scratch stack instead of `MAX_COLLECTION_NESTING + 1` fixed frames; S becomes 88 + `SCRATCH_STACK_BYTES`; D-90 (d) and `StoreLimits` (§6) take that figure in place of the old S, and the memory reservation grows with it; R-SBX-05 and R-SBX-11 gain reason 6; R-SBX-03's exports are unchanged, since the top pointer lives in the base frame.

#### 6.2 JSON and text builtins

- `decode`, `encode_json`, `split`, `to_number` and `header` are emitted WASM, never imports (D-112). They charge from the shared cost constants, and one paid fuel unit still covers a constant number of instructions (R-SBX-05).
- **`decode`** is type-directed: one emitted routine per type reachable from `as`. Types are not recursive, so the routines don't recurse. Skipped members are passed over with a nesting counter and no stack, matching R-RUN-25's iterative walk. Numbers are parsed by emitted code into the canonical slot (D-113), and an inexact one gives `nothing`. A record's fields seen are tracked in a bitset, for the missing-field and duplicate checks.
- **Lists being decoded** don't know their length when they start, so a list pushes a 64-byte frame at its `[` and grows it by 24 bytes per item as the item is appended, with the same budget check per growth (`VL0610`); its item slots collect there until `]`, then move into one charged allocation, and the frame pops. A list inside an item pushes above it and pops before the outer list appends again, so the growing frame is always the top one. The interpreter counts the same 64 + 24 per item at the same points. No `decode` is declined.
- **`encode_json`** measures, then writes, inside the one charge. Numbers are rendered through `velme.to_text` into the frame's marshalling bytes.
- **Memory:** decoded and encoded values are bump-allocated and charged their §7.1 size, the interpreter's figure. Apart from the scratch stack (§6.1), nothing else grows with input size, so D-90's layout and `StoreLimits` gain nothing more.

#### 6.3 Effect operations as host imports

Seven imports join the whitelist under module `velme` (R-SBX-09, §5). A Text argument crosses as the `i32` address of its slot.

| Import | Signature | Notes |
|---|---|---|
| `velme.effect_now_ms` | `() -> n` | the run's frozen clock |
| `velme.effect_fresh_seed` | `() -> n` | derived from the run seed, path and ordinal (§1) |
| `velme.effect_read_text` | `(text: i32) -> i32` | stages the result; returns its ABI size in bytes, or -1 for `nothing` |
| `velme.effect_list_files` | `(text: i32) -> i32` | stages a `List<Text>`; returns its ABI size |
| `velme.effect_fetch_text` | `(text: i32) -> i32` | as `read_text` |
| `velme.effect_fetch` | `(text: i32) -> i32` | stages an `HttpResponse`; returns its ABI size |
| `velme.effect_take` | `(dst: i32) -> i32` | writes the staged result at `dst`; returns the result's slot address |

**One call, in order** (the interpreter's order, §1.3):

1. Emitted code takes the node's entry fuel (1). Out of fuel is `VL0601`, and nothing is performed.
2. Emitted code calls the operation's import. The host reads the argument's slot, checks it against memory bounds (R-SBX-04), checks its length against the fixed 1 024-byte or 2 048-byte cap before reading the bytes, and checks UTF-8; a malformed slot is `VL0607`.
3. The host increments the invocation's ordinal and calls the handler with the invocation path, the ordinal, the operation and the argument. The handler checks the grant and the limits, performs or stubs the operation, and records the event.
4. The host charges the result's fuel (⌈bytes/64⌉ + 1 per item or header) off `velme_fuel_left`, then its §7.1 size off `velme_memory_left`. Either one short traps with the kept `VL0601` or `VL0604`.
5. Emitted code bump-allocates the returned size and calls `velme.effect_take`, which writes the result in the §3 layout, size prefixes included, and returns its slot address. A `dst` out of bounds, a size that doesn't match, or no staged result is `VL0607`.

**Memory: charged like values, not like inputs.** Inputs are uncharged because the caller supplies them before the invocation starts, and D-90 adds their bytes to `StoreLimits` instead. An effect result is produced during the invocation, and the interpreter charges it (§1.3, D-89). So WASM charges the same figure, and the result is a charged value in D-90's sense (a). Its ABI bytes never exceed its charged size (D-53), so `max_memory` already covers it. `StoreLimits` gains no term, and scratch is not used, so the 64 MiB ceiling of `read_max_bytes` needs no frame of that size. The two-step `effect_take` keeps allocation in emitted code: the host never re-enters the module.

**Failures.** `VL0608`, `VL0609`, `VL0803`, `VL0601` and `VL0604` from an effect import are kept errors raised by the host's trap, with `velme_reason` 0 (R-SBX-06, R-SBX-11 (1)). The handler has already recorded the failure event, so the trace matches the interpreter's.

**The handler.** `velme-wasm` takes a `&dyn EffectHandler` in its run call and keeps it in the `Store`'s data. The trait and its value types live in `velme-builtins` (D-114), so `velme-wasm` doesn't depend on `velme-runtime`, and the runtime passes the same real, stub, generated or replay handler it would give the interpreter.

**Path and ordinal.** The runtime passes the leaf's invocation path. The host keeps one ordinal per instance, which is fresh per invocation, and increments it once per operation import before calling the handler, a failing call included. Emitted code evaluates in IR order (R-RUN-02), so (path, ordinal) equal the interpreter's. Seeds, stubs, generated draws, records and replay all key on them, so traces match.

**Watchdog.** Time inside an effect import is excluded from the watchdog, as on the interpreter: the shared watchdog pauses while the handler waits, and the epoch callback asks it (§6 of `runtime/31`).

**Sandbox rules amended:**

- **R-SBX-09:** the whitelist has 22 entries. After `wasmparser` validation and before compiling, the scan also refuses (`VL0801`) an effect import whose capability isn't in the goal's IR `effects`, so a module can't import more than its validated IR declares. The handler checks the grant again at each call. No WASI is linked (R-SBX-10 unchanged).
- **R-SBX-08:** effect imports are the exception to constant host work. Their work before I/O is bounded by the fixed argument caps, and their work after it is proportional to result bytes, which are charged fuel. I/O wait is bounded by `fetch_timeout` and `fetch_max_wait`, and read wait by regular files of `read_max_bytes`.
- **§5:** the `velme.now` Future row is replaced by `velme.effect_now_ms`.
- **R-SBX-17:** under `velme test`, a leaf's examples and generated cases run on the selected backend with the stub or generated handler, so effectful leaves are covered by the differential suite.

#### 6.4 Threat rows (`tooling/41`, R-SEC-14–18)

| Threat | Mitigation |
|---|---|
| T-15 (amended): a module reaches the host through its imports | 22-entry whitelist; effect imports also checked against the IR's `effects`; argument slots bounds-, length- and UTF-8-checked before any I/O; results written only by `effect_take` into memory emitted code allocated, after fuel and memory are charged; no re-entry into the module |
| T-16: a cloned project grants itself network or write access | approval of the grant hash (D-50 precedent); outside entries and wildcards shown prominently |
| T-17: path escape | no `..`; links not followed unless an option says so, and then only into a grant of the same capability; absolute paths only through an approved grant; protected files never writable |
| T-18: server-side request forgery | link-local and metadata refused after DNS and on every redirect hop; literal addresses and known metadata names refused before lookup; with the `env` proxy, Velme resolves each host itself and refuses those answers before sending; plain `http` only to a granted loopback port; wildcards spelled out at approval. Residual with a proxy: the proxy may resolve a name differently from Velme (DNS rebinding between the two lookups, or split-horizon DNS on the proxy's side) |
| T-19: exfiltration of read data through a fetch URL | residual within the approved hosts; approval names them; effect values never reach a provider (R-SEC-08) |
| T-20: resource exhaustion through effects or their config | every limit has a fixed ceiling (`VL0902` above it); monotone run counters; per-attempt timeout and total wait; results charged fuel and memory |
| T-21: hostile responses | body cap after decompression, header count and byte caps, `set-cookie` dropped, terminal escaping of displayed values (T-13) |

#### 6.5 Differential suite (R-SBX §8)

The suite adds, each run on both backends with equal output, trace and figures:

- loop and gate programs;
- local recursion at 256 and 257 frames, fuel running out mid-recursion, and mutual recursion;
- functions applied inside `map`, `filter`, `sort_by` keys and nested collection nodes, including a deep bill-of-materials recursion through comprehensions over long lists, with `VL0610` from the scratch stack at the same push on both backends, its order against `VL0601`, `VL0606` and `VL0604`, and every 0.2 program's figures unchanged;
- `decode_json` over a corpus of malformed JSON, inexact numbers, duplicate keys, deep unknown members and nested lists, plus `encode_json` round trips;
- effectful leaves with stub handlers: every operation, `nothing` results, each failure code mid-body, and memory or fuel running out on a large result;
- the ordinal and path of every effect event, compared across backends.

### 7. Versions and migration (R-SYN-15, R-IR-26, R-ART-27)

**What changes:**

- Only `velme/0.3`, IR `0.3` (new schema `ir-0.3.json`), builtins `0.3`, notation grammar `2` and CLI `0.3.0-alpha.1` are accepted.
- `velme/0.2` gets `VL0106` with help to change the header and rebuild the lock (the D-160 pattern).
- `effects`, `for`, `when`, `choose` and `otherwise` leave the reserved list and become keywords. Using one as a name is now `VL0101`; it was already an error (`VL0104`), so no program breaks. AC-SYN-05 switches to `optional`.
- The `contract_key` budget document gains `max_run_fuel` and `max_recursion_depth`.

**What stays the same:** existing source keeps its meaning. A goal with no `effects` line is pure, and none of the new forms appear unless written.

**The one rebuild:** locks go stale once, through the IR and builtins compatibility units. P4b re-blesses the repository fixtures without provider calls and creates `language/spec/0.3/`.

**Until P4o:** effectful goals can be run only from hand-written IR installed by a test helper (the M3 precedent). P4k–P4n don't claim effectful builds.

### 8. Diagnostics (INV-10, D-186)

| Code | Name | Learner message | Exit |
|---|---|---|---|
| VL0309 | EffectNotDeclared | `{goal}` calls `{child}`, which uses `{effect}` — add `{effect}` to `{goal}`'s `effects` line. | 1 |
| VL0310 | InvalidEffects | `{name}` isn't an effect Velme knows. Effects are: clock, random, read, fetch, write. / This example gives `{op}`, but `{goal}` doesn't use `{effect}`. | 1 |
| VL0311 | BranchMismatch | Every choice in this block must make the same values — `{name}` is missing from the `{arm}` choice. | 1 |
| VL0505 | EffectNotGiven | This example doesn't say what `{call}` gives. Add `given:` with `{call} == …`. | 3 |
| VL0608 | EffectFailed | `{goal}` couldn't {action} `{target}`: {reason}. | 3 |
| VL0609 | EffectLimitExceeded | This run went past `{key}` = {cap} for `{effect}`. / For `fetch_max_wait`: This run spent more than {cap} fetching (depends on network speed). (help: raise `{key}` in `[effects.limits]`, up to {ceiling}; values in human units) | 3 |
| VL0610 | NestingTooDeep | `{goal}`'s code called itself too deeply (more than {cap} levels). / `{goal}`'s code nested too deeply: its lists inside lists needed more than {cap} of working space at once. | 3 |
| VL0611 | RunFuelExceeded | The whole run took too many steps and was stopped. | 3 |
| VL0612 | WriteConflict | Two goals saved different text to `{path}`, so nothing was saved. | 3 |
| VL0802 | EffectNotAllowed | This project doesn't let goals use `{effect}`. / Run `velme allow` to let this project use: {list}. / `{effect}` can't be used with `--offline`. | 3 |
| VL0803 | EffectOutOfScope | `{goal}` tried to {action} `{target}`, which this project doesn't allow. | 3 |
| VL0904 | ReplayUnavailable | This record can't be replayed: {reason}. | 64 |

`VL0605` and `VL0601` keep their meanings: `VL0605` may now come from run-time metering. `VL0801` gains its first IR source, an `effect` node the signature doesn't declare, and a second module source, an effect import the IR doesn't declare. A bad `[effects]`, `[effects.limits]` or `[effects.options]` value is the existing `VL0902`; no new code.

### 9. Invariant text

- **INV-3:** Same source + same inputs + same locked artifacts + same effect results (the run seed and clock included) ⇒ same result and same trace, on every backend. A goal whose call subtree declares no effects is pure: same inputs ⇒ same result. The reference interpreter defines the semantics; any other backend must match it.
- **INV-4:** Goals have no ambient authority. A goal uses only its inputs, its declared calls, whitelisted built-ins and host functions, and the effects its signature declares (`effects`) and the project grants in `velme.toml`, within the user's approval. The runtime refuses any other effect, and build and test perform none.
- **INV-5:** Every execution is bounded: per-invocation fuel, memory, recursion depth, scratch stack, list size and output size; whole-run goal calls, call depth and fuel; per-capability effect operations, bytes, files and fetch time, each configurable up to a fixed ceiling; and a wall-clock watchdog. Limits the source decides are checked statically; the rest are metered deterministically at run time, except fetch time.
- **INV-6:** The set of possible calls is fixed by source before synthesis, and the goal graph is acyclic. A goal calls only goals named in its own `call` block: as a binding, once per item of a `for`, or in the `when`/`choose` arm its source condition selects. Synthesized IR cannot add, remove, reorder or select calls (D-5).
- **P-5** (renamed **Small core first**): recursion inside a goal, loops and conditionals over calls, and effects came in 0.3 (RFC 0002) after the straight-line DAG was stable. Generics and modules still wait.

### 10. Decisions to record

- **D-172:** the owner decisions of 2026-11-03.
- **D-173:** the effects model: capabilities, operations, transitive declaration, grants and approval (§1).
- **D-174:** writing as returned `OutputFile`s, with a staged commit.
- **D-175:** effect config: `[effects.limits]` and `[effects.options]` with defaults and fixed ceilings, absolute and wildcard grant entries, the fixed floor, approval covering limits and options, and "config may widen reach, never change meaning" (§1.2). Replaces the draft's single `max_effect_ops` and 120 s total wait.
- **D-176:** fetch retries: `fetch_retries`, which failures retry, deterministic backoff, `Retry-After`, and how retries count and are traced (§1.7).
- **D-177:** `fetch` returning `HttpResponse`, the `HttpHeader` type and `header`, header caps, `fetch_text`'s mapping, and `VL0608` narrowed to transport failures (§1.7).
- **D-178:** effect verification: `given`, generated results, and build/test performing no effects.
- **D-179:** recording policy and `velme-run/1` replay.
- **D-180:** conditional calls.
- **D-181:** `for`, runtime call metering and `max_run_fuel` with the monotone verdict. Revises D-27, D-51 and AC-RUN-06 (now 10 001 calls).
- **D-182:** local recursion, and the scratch stack for every collection node: frames of 64 + 24 × n, `SCRATCH_STACK_BYTES` = 16 MiB outside `max_memory`, `VL0610` past it, its step in D-88's order (§4.1).
- **D-183:** JSON builtins, `split`, `to_number`, and the prelude types `OutputFile`, `HttpResponse` and `HttpHeader`.
- **D-184:** every new node on WASM in P4: `velme_depth_left` and reason 5, the scratch-stack region with reason 6, emitted JSON, the seven effect imports, charging effect results as values, and the import scan against `effects`. Amends D-90 (d) and §6's `StoreLimits` (S is now the scratch-stack region), D-113 (3) and R-SBX-19 (the scratch stack replaces fixed frames), D-112's whitelist, D-113's exports and R-SBX-08's constant-work rule.
- **D-185:** version 0.3 and the keywords. Amends D-24, notes D-87's `for`, and closes Q-14 with a Velme-specific scheme now and WASI later.
- **D-186:** the diagnostic codes in §8.

New open questions:

- **Q-36:** secrets and authenticated fetch;
- **Q-37:** other HTTP methods, stdout, processes and a live clock;
- **Q-38:** fixture-file stubs;
- **Q-39:** list index, slice and text search builtins;
- **Q-40:** recovering after a transport failure (`fallback`).

### 11. Slices

New acceptance criteria are numbered from the current maxima: AC-SYN-21+, GOAL-19+, TYP-33, CHK-17, BLT-20+, IR-22+, SYNTH-74+, RUN-18+, SBX-10+, ART-17+, SEC-10+ and CLI-31+.

| Slice | Scope (open behaviours) | Tier |
|---|---|---|
| P4a | Spec edits only (this RFC's rules, AC rows, D/Q rows, `reference/90`, `runtime/31`, `tooling/41`). | Opus · high (orchestrator) |
| P4b | Version 0.3: header/config acceptance and `VL0106` help, IR/builtins/notation versions, keywords leave the reserved list, AC-SYN-05, fixture re-bless, CHANGELOG, spec mirror (7). | Sonnet · medium |
| P4c | `when`/`otherwise`/`choose`: grammar, scoping and `T?` typing, `VL0311`, condition lowering and IR, gate scheduling and budget, trace `gate`/`not_taken`, explain, static worst case, key normalization (8). | Opus · high |
| P4d | `for` and run limits: grammar and typing, IR `each`, concurrent iterations under D-9, parent charging, runtime `max_goal_calls`, `max_run_fuel` with `VL0611`, the monotone verdict and truncated trace, explain and index (8). | Opus · high |
| P4e | Local recursion in IR and the interpreter: schema, validator stages 2–4, `apply` fuel and order, `VL0610`, the scratch stack for every collection node and `decode` (counter, budget, its D-88 step, the second `VL0610` message, 0.2 figures unchanged), the R-RUN-25 stack test, golden IR (8). | Opus · high |
| P4f | Notation grammar 2: `def` parsing and lowering, `notation-15`, hints, prompt line, token AC, fuzz (6). | Sonnet · high, then Opus review |
| P4g | Scratch stack on WASM: the region and base frame, frame headers and links, push and pop with reason 6, the host's frame walk for visiting indices and `to_text`, `StoreLimits` and D-90/D-113/R-SBX-19 text, differential cases at the `VL0610` point, the per-run cost measured against D-134's workloads with the 4 MiB fallback decided and recorded (7). | Opus · high |
| P4h | Local recursion on WASM: emitted functions, `velme_depth_left` and reason 5, order and `VL0610` wording, `MAX_FN_LOCALS` and the stack test, recursion through lambdas on the stack, differential cases (6). | Opus · high |
| P4i | `decode`, `encode_json`, `split`, `to_number`, `header`; prelude `OutputFile`, `HttpResponse`, `HttpHeader` with `VL0203`; notation forms, fuel and memory (7). | Sonnet · medium |
| P4j | JSON and text builtins on WASM: type-directed decode, skip counter, number parsing, list frames growing on the scratch stack, two-pass encode, differential corpus (6). | Opus · high |
| P4k | Effect declarations: grammar, `VL0310`, transitive `VL0309`, signature/key/manifest `effects`, IR `effects` and `effect` node at stages 3/5/6, prompt section with `HttpResponse` only under `fetch`, explain (8). | Opus · high |
| P4l | Effect runtime I, config and approval: `EffectHandler`, `[effects]` with absolute and wildcard entries, `[effects.limits]`/`[effects.options]` with ceilings and `VL0902`, approval hash and `approvals.toml`, `velme allow` display, `--allow-effects`, `VL0802` and `--offline` (7). | Opus · high (security) |
| P4m | Effect runtime II, local effects: clock, `--seed` and random derivation, read and list path sandbox with the symlink options, run counters and `VL0609`, staging and commit with `VL0612` and write limits, `saved` output, watchdog exclusion (8). | Opus · high (security) |
| P4n | Effect runtime III, fetch: host rules and the floor, redirects, proxy option, retries and backoff with `Retry-After`, `fetch_max_wait`, `HttpResponse` building and header caps, `fetch_text` mapping with `VL0608`/`VL0803`, threat rows T-16–21 (8). | Opus · high (security) |
| P4o | Verification with effects: `given` grammar and typing, stub handler, `VL0505`, generated results with `HttpResponse` statuses, R-ART-28 input string, prompt `given` cap, re-verification (7). | Opus · medium |
| P4p | Effect imports on WASM: the seven imports and their marshalling, charging order, `effect_take`, handler in `Store` data, path and ordinal, import scan against `effects`, R-SBX-08/09 and T-15 text, differential cases with stub handlers (8). It gets its own architect review before commit. | Opus · high (security) |
| P4q | Recording and replay: trace `effect` events, `[effects.record]`, `--record`, `--replay`/`--live`, `VL0904`, goldens (6). The format is a public contract, so it gets an architect review before commit. | Sonnet · medium |

## Alternatives and drawbacks

- **Effects only as compiler-provided goals in `call` blocks.**
  - Gain: every body stays pure, and INV-6 needs no rewording.
  - Loss: URLs and paths would need a goal just to compute them (R-GOAL-08 allows only paths and literals as arguments), and data-dependent sequences such as pagination can't be written.
  - Writes keep this model's main benefit: they are verifiable data.
- **A `write_text` operation.** Rejected because checks and examples couldn't see the write, so a wrong path would pass verification. A failed run would need rollback.
- **Static per-loop bounds (`budget items=N`).** This keeps D-51 purely static. Rejected because every loop would need a number, and multiplying static bounds rejects ordinary programs. The monotone verdict gives the same determinism without either.
- **A `while`/`loop` IR node.** Rejected because recursion plus `reduce` over `range` cover iteration, and it would need reassignment in the notation (R-SYNTH-51).
- **Recursion through goal self-calls.** Rejected because it breaks R-GOAL-11 and the D-11 keys, and it spends goal calls and budgets on every frame.
- **Project-only grants.** Rejected because a cloned project could grant itself network and write access (T-10 / D-50 precedent).
- **Fixed effect limits.** Rejected by the owner: real projects read and fetch more than 256 times. Fixed ceilings and approval keep the bound.
- **`fetch_text` only, with `VL0608` on any other status.** Rejected by the owner: APIs signal state through statuses, and a program must be able to see them.
- **Declining the new nodes on WASM in P4.** Rejected by the owner: effectful leaves would never run on WASM, and the gap would grow with each new node.
- **Declining an `apply` inside a lambda on WASM.** Rejected by the owner: calling a local function from `map` is ordinary code.
- **Saving busy frames on such an `apply`, charged as memory** (d × a full frame, or only the busy part). Rejected by the owner: memory charges are cumulative (D-53), so a bill of materials whose every recursive call scans a 10 000-row list pays once per call, which is quadratic in practice; a full-frame save failed a `map` after about 280 items.
- **A per-call or per-evaluation memory charge for scratch.** Rejected for the same quadratic reason.
- **Frame offsets from call sites.** Rejected: offsets can't handle a recursive call inside a lambda.
- **Host re-entry into `velme_alloc` for effect results.** Rejected in favour of `effect_take`: re-entry runs module code inside a host call and on Wasmtime fuel of its own.
- **WASI capabilities (Q-14).** Effects run on the host for now, through Velme's own imports. WASI belongs to a later RFC.

**Drawbacks and risks:**

1. All locks rebuild once (paid synthesis for each user).
2. An HTTPS client now sits in `velme-runtime` (T-6).
3. A truncated trace on a run-cap failure is less helpful.
4. 256 frames limits linear recursion over long lists.
5. WASM gains seven effectful imports, new attack surface for T-15.
9. The scratch stack bounds recursion through a lambda by list length: about 69 levels over 10 000-item lists. The WASM memory reservation grows by about 15 MiB, since the 16 MiB region replaces the fixed frames' 1.2 MiB.
6. The trace already holds input and call values in full, so the digest policy is not a privacy guarantee.
7. Token cost: the `ir-json` schema grows by about 1 KB (4 node kinds); notation requests carry no schema; the fixed prefix grows by about 180 bytes and may need AC-SYNTH-55's 5 120-byte bound raised to 5 376; effectful goals pay about 25 tokens per operation, and `fetch` goals also pay for the `HttpResponse` definition.
8. Configurable limits let a project ask for a 1 h fetch budget and 1 GiB of writes; approval shows such values, but a user may approve without reading.

## Effect on invariants

| Invariant | Effect |
|---|---|
| INV-1, INV-2 | Intact. Effects are validated IR nodes. Checks never call effects, and their verdicts stay deterministic, since examples and generation supply the effect results. |
| INV-3 | Narrowed as worded in §9, and full for pure subtrees. Kept by the run seed and per-path derivation, the frozen clock, staged writes, the monotone verdicts, one handler and one ordinal sequence for both backends, and the differential suite. Config changes reach, not results. |
| INV-4 | Amended to capabilities (§9). Enforced at the validator (stage 5), at the WASM import scan, before the run (grant and approval) and at each operation (scope and floor). |
| INV-5 | Extended (§9). |
| INV-6 | Reworded (§9). R-GOAL-11 is unchanged. |
| INV-7 | Kept: check, build and test need no network. Only a run whose program fetches does, and `--offline` refuses it. |
| INV-8 | Kept: `effects` and `given` enter keys; grants, limits, options and record policy don't. |
| INV-9 | No new crate edge. `velme-wasm` reaches the handlers through the `velme-builtins` trait. |
| INV-10 | Twelve new codes; no code is reused. Config errors reuse `VL0902` in its existing meaning. |

## Spec gaps and contradictions found

1. D-87 reserved `for` for example groups (Q-26). P4 uses it for call loops; Q-26 needs a contextual `for` in `examples:` or another word.
2. R-SEC-04 lists `storage, network`, which this RFC renames to `read`/`write`/`fetch`.
3. R-RUN-25 says recursion is "statically bounded". Local recursion is bounded at run time.
4. D-51's static bound assumes a static call graph. D-51 rejected a run envelope; D-181 revisits that at a cap ordinary programs can't reach.
5. R-SEC-08 ("data goes to exactly one place") is false once `fetch` exists and needs a run-time carve-out.
6. Under D-23, strict decoding would make `decode_json` useless on real APIs. The lenient rule is a deliberate difference.
7. Q-24 (keywords as field names) now also blocks decoding API JSON with keys like `type`.
8. R-IR-15 calls stage 5 a guard against schema extensions only. It now has a real node to guard.
9. The notation's `notation-4` hint for `def` ("write only the body") becomes wrong in grammar 2.
10. R-SBX-09 says no effectful host function exists, and §5 lists `velme.now` as Future requiring `effects: clock`. Both change with the effect imports.
11. `runtime/31` §6 says emitted code never recurses. `apply` makes it recurse, bounded by `velme_depth_left`.
12. R-SBX-08 requires constant host work per import. Effect imports do I/O, so they need the carve-out of §6.3.
13. T-15 counts a 15-entry whitelist; it becomes 22.
14. D-113 sized one fixed frame per collection nesting depth, which recursion through a lambda breaks. The scratch stack replaces it, and `runtime/30` §7 says scratch frames are uncharged, which stays true.
