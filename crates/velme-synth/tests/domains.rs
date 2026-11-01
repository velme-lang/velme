//! Domain values (`compiler/22` §7.5, D-161, D-162, D-164): the finite sets, value-level retries, record conditions,
//! the scarcity probe, and the golden vectors V4–V6, which a conforming generator reproduces exactly (R-SYNTH-66).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::Value;
use velme_check::{Invocation, Membership};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Fingerprint, ValidIr, decode_str, wired_goal};
use velme_sema::hir::{DomainId, Program, Type};
use velme_synth::{
    ChildRunner, PROBE_DRAWS, Probe, TestInput, TestInputs, Verdict, goal_inputs, probe, probe_domain,
    probe_domain_within, test_inputs, test_inputs_within, value_sets, verify,
};
use velme_test_support::{goal_id, program, valid_ir};

const ZERO: &str = "b3:0000000000000000000000000000000000000000000000000000000000000000";
const KEY: &str = "b3:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const KEY2: &str = "b3:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// `decls`, then one goal `G` with `params`.
fn source(decls: &str, params: &str) -> String {
    format!("language: velme/0.2\n\n{decls}\n\ngoal G({params}) -> Number:\n    plan: \"Use them.\"\n")
}

fn generate(src: &str, key: &str) -> TestInputs {
    let program = program(src);
    let key: Fingerprint = key.parse().expect("a fingerprint");
    test_inputs(&program, goal_id(&program, "G"), src, key, &[]).expect("inputs")
}

fn stages(inputs: &TestInputs) -> (usize, usize, usize, usize, u64) {
    (
        inputs.examples,
        inputs.boundary,
        inputs.pairwise,
        inputs.random,
        inputs.retries,
    )
}

fn lines(inputs: &TestInputs) -> Vec<&str> {
    inputs.inputs.iter().map(|i| i.json.as_str()).collect()
}

/// The type of `G`'s parameter `i`.
fn param(program: &Program, i: usize) -> Type {
    program.goals[goal_id(program, "G").0].params[i].ty.clone()
}

/// `B` and `S` of `G`'s parameter `i`.
fn sets(src: &str, i: usize) -> (Vec<String>, Vec<String>) {
    let program = program(src);
    value_sets(&program, src, &param(&program, i)).expect("sets")
}

fn json(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// The probe of the domain declared `n`th in `src`.
fn probed(src: &str, n: usize) -> Probe {
    let program = program(src);
    let membership = Membership::new(&program, src).expect("membership");
    probe_domain(&program, &membership, DomainId(n)).expect("a probe")
}

/// The first `n` probe draws, an invalid one marked `✗`.
fn draws(probe: &Probe, n: usize) -> Vec<String> {
    probe
        .draws
        .iter()
        .take(n)
        .map(|d| {
            let value = d.value.clone().unwrap_or_else(|| "-".to_owned());
            if d.valid { value } else { format!("{value}✗") }
        })
        .collect()
}

/// The first eight digest bytes of `hash`, little-endian (R-SYNTH-65).
fn seed_of(hash: Fingerprint) -> u64 {
    let hex = hash.hex();
    let bytes: Vec<u8> = (0..8)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    u64::from_le_bytes(bytes.try_into().expect("8 bytes"))
}

/// The `j`-th output of the SplitMix64 stream seeded with `seed` (§7.3), computed here independently.
fn output(seed: u64, j: u64) -> u64 {
    let mut z = seed.wrapping_add(j.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `below(n)` of output `j` of the stream seeded with `stream` (§7.3).
fn below(stream: u64, j: u64, n: u128) -> u128 {
    (u128::from(output(stream, j)) * n) >> 64
}

/// §7.3's Number draw from the stream `stream`, next output `j`, in hundredths.
fn base_hundredths(stream: u64, j: &mut u64) -> i128 {
    let mut next = |n: u128| {
        *j += 1;
        below(stream, *j, n) as i128
    };
    if next(2) == 0 {
        (next(2001) - 1000) * 100
    } else {
        next(200_001) - 100_000
    }
}

/// `hundredths / 100` as canonical JSON.
fn decimal(hundredths: i128) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let (whole, cents) = (hundredths.abs() / 100, hundredths.abs() % 100);
    let cents = format!("{cents:02}");
    match cents.trim_end_matches('0') {
        "" => format!("{sign}{whole}"),
        cents => format!("{sign}{whole}.{cents}"),
    }
}

/// The stream of attempt `i` (from 0) under `seed` (§7.3).
fn attempt(seed: u64, i: u64) -> u64 {
    output(seed, i + 1)
}

/// `values`' fingerprint: how a vector's whole list is frozen, the digest verification records for an input set
/// (`runtime/32` §3).
fn frozen(values: &[impl AsRef<str>]) -> String {
    let values: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    Fingerprint::of(&values).expect("a fingerprint").to_string()
}

/// Every generated value of `G`'s domain parameter `i` is a member, by `velme-check`'s predicate (R-CHK-15).
fn assert_members(src: &str, inputs: &TestInputs, i: usize) {
    let program = program(src);
    let membership = Membership::new(&program, src).expect("membership");
    let Type::Domain(id, _) = param(&program, i) else {
        panic!("parameter {i} is a domain")
    };
    for input in &inputs.inputs {
        assert!(membership.member(id, &input.args[i]).is_ok(), "{}", input.json);
    }
}

// ---- golden vectors V4–V6 (R-SYNTH-66, AC-SYNTH-68) ----

const V4: &str = "language: velme/0.2

type Risk = Text one of \"B\", \"A\"
type Solo = Text one of \"X\"

goal G(risk: Risk, solo: Solo) -> Number:
    plan: \"Rate it.\"
";

const V5: &str = "language: velme/0.2

type Age = Number where value > 18 and value <= 20 and value is whole
type Code = Text where value.length > 0 and value.length < 3

goal G(age: Age, code: Code) -> Boolean:
    plan: \"Admit.\"
";

const V6: &str = "language: velme/0.2

type Span where value.x < value.y:
    x: Number
    y: Number

goal G(span: Span) -> Number:
    plan: \"Width.\"
";

/// AC-SYNTH-68, V4: a closed set's `B` and `S` are its members in order; a singleton's 64 probe draws all succeed.
#[test]
fn ac_synth_68_golden_vector_v4() {
    assert_eq!(sets(V4, 0), (json(&["\"B\"", "\"A\""]), json(&["\"B\"", "\"A\""])));
    assert_eq!(sets(V4, 1), (json(&["\"X\""]), json(&["\"X\""])));
    let risk = probed(V4, 0);
    assert_eq!(
        risk.declaration_hash.to_string(),
        "b3:0846652a752844479df914980ff87ce79a0afdc390233ad57d8480342a056496"
    );
    assert_eq!(risk.seed, 0x4744_2875_2a65_4608);
    assert_eq!(risk.valid(), 64);
    assert_eq!(
        draws(&risk, 8),
        ["\"B\"", "\"B\"", "\"A\"", "\"A\"", "\"A\"", "\"A\"", "\"B\"", "\"A\""]
    );
    // F3 of D5: every draw is `below(2)` over the members, computed here independently, and the whole list is frozen.
    for (i, draw) in risk.draws.iter().enumerate() {
        let member = ["\"B\"", "\"A\""][(output(attempt(risk.seed, i as u64), 1) >> 63) as usize];
        assert_eq!(draw.value.as_deref(), Some(member), "draw {i}");
    }
    assert_eq!(
        frozen(&draws(&risk, 64)),
        "b3:e4410dc4d42eee86d0afa13f410bd97c9158513ff86f764f617f91605451e24b"
    );
    let solo = probed(V4, 1);
    assert_eq!(
        solo.declaration_hash.to_string(),
        "b3:a4551115242dbe5821e36b312c29189bef52b3223c644ed21b9da0e1bbac848a"
    );
    assert_eq!(solo.seed, 0x58be_2d24_1511_55a4);
    assert_eq!(solo.valid(), 64, "duplicate successes count");
    assert!(solo.draws.iter().all(|d| d.value.as_deref() == Some("\"X\"")));
    // Two distinct inputs exist: the boundary stage takes both, and every random attempt repeats one.
    let inputs = generate(V4, ZERO);
    assert_eq!(stages(&inputs), (0, 2, 0, 0, 0));
    assert_eq!(
        lines(&inputs),
        [r#"{"risk":"B","solo":"X"}"#, r#"{"risk":"A","solo":"X"}"#]
    );
}

/// AC-SYNTH-68, V5: projection, edges and de-duplication give `B = S = [19, 20]`; Text length 1–2 has edges `"a"` and
/// `"aa"`.
#[test]
fn ac_synth_68_golden_vector_v5() {
    assert_eq!(sets(V5, 0), (json(&["19", "20"]), json(&["19", "20"])));
    assert_eq!(
        sets(V5, 1),
        (
            json(&["\"a\"", "\"Li\"", "\"é🙂\"", "\"aa\""]),
            json(&["\"a\"", "\"aa\""])
        )
    );
    let age = probed(V5, 0);
    assert_eq!(
        age.declaration_hash.to_string(),
        "b3:6684e28443f5e3a1e0360f23df9b01cc6f63c21e6eb7019baa04a89bae475465"
    );
    assert_eq!(age.seed, 0xa1e3_f543_84e2_8466);
    assert_eq!(age.valid(), 64);
    assert_eq!(draws(&age, 8), ["20", "20", "20", "20", "20", "19", "19", "19"]);
    // F3 of D5: every draw is `19 + below(2)`, computed here independently, and the whole list is frozen.
    for (i, draw) in age.draws.iter().enumerate() {
        let n = 19 + (output(attempt(age.seed, i as u64), 1) >> 63);
        assert_eq!(draw.value.as_deref(), Some(n.to_string().as_str()), "draw {i}");
    }
    assert_eq!(
        frozen(&draws(&age, 64)),
        "b3:ccc34c3fa40ba150c8be81d03b0448504e6e68672e0a221e88746c4523f04d88"
    );
    let code = probed(V5, 1);
    assert_eq!(
        code.declaration_hash.to_string(),
        "b3:c69deaab9f8e41d9dea47d1cd41704d81f18ed682f50df8c025e6dc9eb7a0dc4"
    );
    assert_eq!(code.seed, 0xd941_8e9f_abea_9dc6);
    assert_eq!(code.valid(), 64);
    assert_eq!(
        draws(&code, 8),
        [
            "\"o\"", "\"t\"", "\"w\"", "\"aw\"", "\"m\"", "\"qt\"", "\"0y\"", "\"rl\""
        ]
    );
    // F3 of D5: the length is `1 + below(2)`, then each scalar `below(32)` of §7.3's alphabet.
    let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyzAZ0 \u{e9}\u{1f642}".chars().collect();
    for (i, draw) in code.draws.iter().enumerate() {
        let stream = attempt(code.seed, i as u64);
        let length = 1 + below(stream, 1, 2) as u64;
        let text: String = (0..length)
            .map(|at| alphabet[below(stream, 2 + at, 32) as usize])
            .collect();
        assert_eq!(draw.value.as_deref(), Some(format!("\"{text}\"").as_str()), "draw {i}");
    }
    assert_eq!(
        frozen(&draws(&code, 64)),
        "b3:4c78a26e6fd2a6ab44d400dc8bf666704e5999f94f97a08ba058531b6ab1e6ca"
    );
    let inputs = generate(V5, KEY);
    assert_eq!(stages(&inputs), (0, 4, 2, 58, 0));
    assert_eq!(
        lines(&inputs)[..10],
        [
            r#"{"age":19,"code":"a"}"#,
            r#"{"age":20,"code":"Li"}"#,
            r#"{"age":19,"code":"é🙂"}"#,
            r#"{"age":20,"code":"aa"}"#,
            r#"{"age":19,"code":"aa"}"#,
            r#"{"age":20,"code":"a"}"#,
            r#"{"age":20,"code":"b"}"#,
            r#"{"age":19,"code":"c"}"#,
            r#"{"age":20,"code":"éy"}"#,
            r#"{"age":19,"code":"vl"}"#,
        ]
    );
    assert_eq!(
        frozen(&lines(&inputs)),
        "b3:d4017ea97ab107c6388fe6041406c41df2b77264e138ee7b11957cc408def8b4"
    );
    assert_members(V5, &inputs, 0);
    assert_members(V5, &inputs, 1);
}

/// AC-SYNTH-68, AC-SYNTH-66, V6: the base fields' diagonal pairs are equal, so `value.x < value.y` empties `B` and
/// `S`; both stages are skipped, and random records, redrawn whole, fill every slot.
#[test]
fn ac_synth_68_golden_vector_v6() {
    assert_eq!(sets(V6, 0), (Vec::new(), Vec::new()));
    let span = probed(V6, 0);
    assert_eq!(
        span.declaration_hash.to_string(),
        "b3:85fb4b381ff9147d49c87009529c3e32b30011efb5320f51ad11e9d7739c88e4"
    );
    assert_eq!(span.seed, 0x7d14_f91f_384b_fb85);
    assert_eq!(span.valid(), 29);
    assert_eq!(
        draws(&span, 8),
        [
            r#"{"x":-928,"y":-528.57}"#,
            r#"{"x":-305,"y":768}"#,
            r#"{"x":-775.32,"y":97.74}"#,
            r#"{"x":495,"y":-369}✗"#,
            r#"{"x":682,"y":925.1}"#,
            r#"{"x":101.56,"y":-66}✗"#,
            r#"{"x":375.02,"y":549.91}"#,
            r#"{"x":-573.25,"y":-949}✗"#,
        ]
    );
    // F3 of D5: each draw is §7.3's Number draw for `x`, then for `y`, valid when `x < y`; computed here independently.
    for (i, draw) in span.draws.iter().enumerate() {
        let (stream, mut j) = (attempt(span.seed, i as u64), 0);
        let (x, y) = (base_hundredths(stream, &mut j), base_hundredths(stream, &mut j));
        let value = format!(r#"{{"x":{},"y":{}}}"#, decimal(x), decimal(y));
        assert_eq!(draw.value.as_deref(), Some(value.as_str()), "draw {i}");
        assert_eq!(draw.valid, x < y, "draw {i}");
    }
    assert_eq!(
        frozen(&draws(&span, 64)),
        "b3:2622abf4aa792958a07671074087d8856625a1ad7f24e87f071da8c8ca6de64b"
    );
    let inputs = generate(V6, KEY2);
    assert_eq!(stages(&inputs), (0, 0, 0, 64, 60));
    assert_eq!(
        lines(&inputs)[..4],
        [
            r#"{"span":{"x":-621.2,"y":337}}"#,
            r#"{"span":{"x":-850.13,"y":19}}"#,
            r#"{"span":{"x":-870.61,"y":249}}"#,
            r#"{"span":{"x":-171,"y":227.02}}"#,
        ]
    );
    // F3 of D5: the same draws under the key's seed, a whole record redrawn until `x < y`, give every line and retry.
    let (seed, mut expected, mut retries) = (seed_of(KEY2.parse().expect("a fingerprint")), Vec::new(), 0);
    for i in 0..64 {
        let (stream, mut j) = (attempt(seed, i), 0);
        for candidate in 0..17 {
            let (x, y) = (base_hundredths(stream, &mut j), base_hundredths(stream, &mut j));
            if x < y {
                expected.push(format!(r#"{{"span":{{"x":{},"y":{}}}}}"#, decimal(x), decimal(y)));
                retries += candidate;
                break;
            }
        }
    }
    assert_eq!(lines(&inputs), expected);
    assert_eq!(inputs.retries, retries);
    assert_eq!(
        frozen(&lines(&inputs)),
        "b3:5e039283b84abe537540ba769a4b1c1e5f6f68d75f3748a41b86f920d6acef94"
    );
    assert_members(V6, &inputs, 0);
}

/// AC-SYNTH-37: a program without domains consumes the stream as before; V1 still reproduces (`tests/generate.rs` has
/// V1–V3 in full).
#[test]
fn ac_synth_68_a_program_without_domains_is_unchanged() {
    let v1 = "language: velme/0.2\n\ngoal G(n: Number) -> Number:\n    plan: \"Double it.\"\n";
    let inputs = generate(
        v1,
        "b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    );
    assert_eq!(stages(&inputs), (0, 6, 0, 58, 0));
    assert_eq!(
        lines(&inputs)[6..9],
        [r#"{"n":-502.7}"#, r#"{"n":-756}"#, r#"{"n":-710}"#]
    );
}

// ---- value sets (R-SYNTH-62, AC-SYNTH-64) ----

/// `one of "m1", …, "m{n}"`.
fn members(n: usize) -> String {
    (1..=n).map(|i| format!("\"m{i}\"")).collect::<Vec<_>>().join(", ")
}

/// AC-SYNTH-64, AC-SYNTH-66: a closed set's `B` is its first 16 members and `S` its first two (or its sole one); a set
/// of at most 16 has every member in the boundary stage, and random draws still use all 17 of a larger one.
#[test]
fn ac_synth_64_closed_sets_of_1_16_and_17_members() {
    let one = source("type M = Text one of \"only\"", "m: M");
    assert_eq!(sets(&one, 0), (json(&["\"only\""]), json(&["\"only\""])));
    let sixteen = source(&format!("type M = Text one of {}", members(16)), "m: M");
    let (b, s) = sets(&sixteen, 0);
    assert_eq!(b.len(), 16);
    assert_eq!(s, json(&["\"m1\"", "\"m2\""]));
    let inputs = generate(&sixteen, KEY);
    assert_eq!(inputs.boundary, 16, "every member is covered");
    assert_eq!(lines(&inputs)[15], r#"{"m":"m16"}"#);
    let seventeen = source(&format!("type M = Text one of {}", members(17)), "m: M");
    let (b, s) = sets(&seventeen, 0);
    assert_eq!(b, (1..=16).map(|i| format!("\"m{i}\"")).collect::<Vec<_>>());
    assert_eq!(s, json(&["\"m1\"", "\"m2\""]));
    let inputs = generate(&seventeen, KEY);
    assert_eq!((inputs.boundary, inputs.pairwise, inputs.random), (16, 0, 1));
    assert_eq!(
        lines(&inputs)[16],
        r#"{"m":"m17"}"#,
        "random draws reach the 17th member"
    );
}

/// AC-SYNTH-64: Number sets project the base values into inclusive, exclusive, whole and negative bounds and append
/// both edges; an interval narrower than 0.01 uses its midpoint; random values lie inside.
#[test]
fn ac_synth_64_number_bounds_give_both_edges() {
    for (decl, b, s) in [
        (
            "value >= 0 and value < 100",
            &["0", "1", "0.5", "99.99"][..],
            &["0", "1", "99.99"][..],
        ),
        (
            "value > -1 and value < 1",
            &["0", "0.99", "-0.99", "0.5"],
            &["0", "0.99", "-0.99"],
        ),
        (
            "value > -3.5 and value < 7.2 and value is whole",
            &["0", "1", "-1", "7", "-3"],
            &["0", "1", "-1", "-3", "7"],
        ),
        ("value < -5 and value is whole", &["-6", "-1000000"], &["-6"]),
        ("value > 0", &["0.01", "1", "0.5", "1000000"], &["0.01", "1"]),
        ("value > 0 and value < 0.001", &["0.0005"], &["0.0005"]),
        // F4 of D5: bound + 0.01 is the other bound, not inside, so the midpoint is the edge though no narrower.
        ("value > 0 and value < 0.01", &["0.005"], &["0.005"]),
    ] {
        let src = source(&format!("type D = Number where {decl}"), "d: D");
        assert_eq!(sets(&src, 0), (json(b), json(s)), "{decl}");
        let inputs = generate(&src, KEY);
        // Every draw of the narrow interval is its midpoint, already a boundary input.
        assert!(inputs.random > 0 || b.len() == 1, "{decl}");
        assert_members(&src, &inputs, 0);
    }
}

/// AC-SYNTH-64: Text lengths 5 through 100 draw lengths 5 through 21 only, while the boundary set still has length
/// 100; a lower bound too long for any input leaves no value, checked before anything that size is built.
#[test]
fn ac_synth_64_text_length_bounds() {
    let src = source("type T = Text where value.length >= 5 and value.length <= 100", "t: T");
    let hundred = format!("\"{}\"", "a".repeat(100));
    assert_eq!(
        sets(&src, 0),
        (
            json(&["\"aaaaa\"", "\"Linaa\"", "\"é🙂aaa\"", &hundred]),
            json(&["\"aaaaa\"", &hundred])
        )
    );
    let inputs = generate(&src, KEY);
    let random = &inputs.inputs[inputs.boundary + inputs.pairwise..];
    assert_eq!(random.len(), inputs.random);
    for input in random {
        let Value::Text(t) = &input.args[0] else { panic!("text") };
        assert!((5..=21).contains(&t.chars().count()), "{}", input.json);
    }
    assert_members(&src, &inputs, 0);
    let huge = source("type T = Text where value.length >= 300000", "t: T");
    assert_eq!(sets(&huge, 0), (Vec::new(), Vec::new()));
    assert_eq!(stages(&generate(&huge, KEY)), (0, 0, 0, 0, 0));
}

// ---- residual conditions (R-SYNTH-63, AC-SYNTH-65) ----

/// Never a member: its residual excludes its whole interval.
const NEVER: &str =
    "type Never = Number where value >= 1 and value <= 6 and value is whole and (value > 100 or value < 0)";

/// AC-SYNTH-65: a condition no value meets empties the finite sets and gives each random value 17 candidates, 16 of them
/// retries, before the attempt is skipped; all 256 attempts count.
#[test]
fn ac_synth_65_an_all_rejected_filter_makes_17_candidates_per_draw() {
    let src = source(NEVER, "n: Never");
    assert_eq!(sets(&src, 0), (Vec::new(), Vec::new()));
    assert_eq!(stages(&generate(&src, KEY)), (0, 0, 0, 0, 256 * 16));
}

/// AC-SYNTH-65: a failed child draw fails its container: a record field, and a record domain whose own condition is
/// then never evaluated, so no record-level retry is added to the child's 16.
#[test]
fn ac_synth_65_a_failed_child_draw_fails_its_container() {
    let field = source(&format!("{NEVER}\ntype Box:\n    n: Never"), "b: Box, k: Number");
    assert_eq!(stages(&generate(&field, KEY)), (0, 0, 0, 0, 256 * 16));
    let domain = source(
        &format!("{NEVER}\ntype Wrapped where value.n >= 0:\n    n: Never"),
        "w: Wrapped",
    );
    assert_eq!(stages(&generate(&domain, KEY)), (0, 0, 0, 0, 256 * 16));
}

/// AC-SYNTH-65: a residual condition filters a scalar value, redrawing only that value: a sibling drawn before it
/// keeps the value the same stream gives it without the domain.
#[test]
fn ac_synth_65_only_the_failed_value_is_redrawn() {
    let decl = "type Two = Number where value >= 1 and value <= 6 and value is whole and (value == 2 or value == 5)";
    let src = source(decl, "a: Number, d: Two");
    // The projected base set is `[1, 6]`: filtering empties it and never substitutes another value.
    assert_eq!(sets(&src, 1), (Vec::new(), Vec::new()));
    let filtered = generate(&src, KEY);
    assert!(filtered.retries > 0);
    assert_members(&src, &filtered, 1);
    let plain = generate(&source("", "a: Number, d: Number"), KEY);
    let first = |inputs: &TestInputs| inputs.inputs[inputs.boundary + inputs.pairwise].args[0].clone();
    assert_eq!(first(&filtered), first(&plain));
}

/// AC-SYNTH-65: a condition's value error rejects the value rather than failing generation.
#[test]
fn ac_synth_65_a_value_error_rejects_the_candidate() {
    let src = source(
        "type Inverse = Number where value >= 0 and value <= 3 and value is whole and 6 / value > 1",
        "n: Inverse",
    );
    assert_eq!(sets(&src, 0), (json(&["1", "3"]), json(&["1", "3"])));
    let inputs = generate(&src, KEY);
    assert!(inputs.inputs.iter().all(|i| i.json != r#"{"n":0}"#));
    assert_members(&src, &inputs, 0);
}

/// A predicate that runs past the system budget.
const BUSY: &str = "type Busy = Number where every i in range(10000) has every j in range(10000) has i + j >= value";

/// AC-SYNTH-65, AC-SYNTH-67, F2 and F7 of D5: a predicate out of budget keeps its code, in generation, before any
/// verification (no verdict on the candidate), and in the probe, which doesn't report it as scarcity; each worded for
/// the domain and placed at its declaration, the goal a note in generation.
#[test]
fn ac_synth_65_budget_exhaustion_keeps_its_code() {
    let src = source(BUSY, "n: Busy");
    let program = program(&src);
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let declared = src.find("type Busy").expect("the declaration")..src.find("goal G").expect("the goal");
    let worded = |d: &Diagnostic| {
        assert_eq!(d.code, Code::BudgetExceeded, "{d:?}");
        assert_eq!(d.message, "Checking whether a value is a `Busy` took too many steps.");
        assert_eq!(d.help.as_deref(), Some("make its `where` condition cheaper"));
        assert!(declared.contains(&d.span.start), "{d:?}");
    };
    let error = test_inputs(&program, goal_id(&program, "G"), &src, key, &[]).expect_err("out of budget");
    worded(&error);
    let error = goal_inputs(&program, goal_id(&program, "G"), &src, key).expect_err("out of budget");
    worded(&error);
    assert!(error.notes.iter().any(|n| n.contains("`G`")), "{error:?}");
    let found = probe(&program, &src);
    assert_eq!(found.len(), 1, "{found:?}");
    worded(&found[0]);
    assert!(found[0].notes.iter().all(|n| !n.contains("`G`")), "{found:?}");
}

// ---- empty sets and examples (R-SYNTH-64, AC-SYNTH-66) ----

/// AC-SYNTH-66: an empty item set leaves `[nothing]` for an optional and `[[]]` for a list, so their stages still run;
/// an empty record diagonal empties a parameter diagonal and skips both stages, never a modulus by zero.
#[test]
fn ac_synth_66_empty_sets_propagate() {
    let src = source(NEVER, "xs: List<Never>, o: Never?, n: Number");
    assert_eq!(sets(&src, 0), (json(&["[]"]), json(&["[]"])));
    assert_eq!(sets(&src, 1), (json(&["null"]), json(&["null"])));
    let inputs = generate(&src, KEY);
    assert_eq!((inputs.boundary, inputs.pairwise), (6, 0));
    assert_eq!(lines(&inputs)[0], r#"{"n":0,"o":null,"xs":[]}"#);
    let record = source(
        &format!("{NEVER}\ntype Box:\n    n: Never\n    m: Number"),
        "b: Box, k: Number",
    );
    assert_eq!(sets(&record, 0), (Vec::new(), Vec::new()));
    let inputs = generate(&record, KEY);
    assert_eq!((inputs.boundary, inputs.pairwise), (0, 0));
}

/// AC-SYNTH-66: an example holding a member changes neither `B` nor `S`, nor the candidates' order; only the existing
/// de-duplication drops the boundary input it repeats.
#[test]
fn ac_synth_66_examples_only_deduplicate() {
    let src = source("type Risk = Text one of \"B\", \"A\", \"C\"", "risk: Risk");
    let program = program(&src);
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let example = vec![vec![Value::text("A")]];
    let inputs = test_inputs(&program, goal_id(&program, "G"), &src, key, &example).expect("inputs");
    assert_eq!(sets(&src, 0).0, json(&["\"B\"", "\"A\"", "\"C\""]));
    assert_eq!(
        lines(&inputs),
        [r#"{"risk":"A"}"#, r#"{"risk":"B"}"#, r#"{"risk":"C"}"#]
    );
    assert_eq!((inputs.examples, inputs.boundary), (1, 2));
}

// ---- the probe (R-SYNTH-65, AC-SYNTH-67, AC-SYNTH-70) ----

/// `Rare` with `value <= k` as its residual's first disjunct.
fn rare(k: u32) -> String {
    source(
        &format!(
            "type Rare = Number where value >= 1 and value <= 64 and value is whole and (value <= {k} or value > 1000)"
        ),
        "n: Rare",
    )
}

/// How many of `probe`'s 64 draws of `1 + below(64)` are at most `k`, computed here independently (F3 of D5).
fn at_most(probe: &Probe, k: u128) -> usize {
    (0..PROBE_DRAWS)
        .filter(|i| below(attempt(probe.seed, *i), 1, 64) < k)
        .count()
}

/// AC-SYNTH-67: 64 raw draws, without the domain's own retries; 7 valid ones is `VL0210` with `7/64`, 8 pass.
#[test]
fn ac_synth_67_thresholds_7_and_8() {
    let seven = rare(4);
    let probe7 = probed(&seven, 0);
    assert_eq!((probe7.draws.len(), probe7.valid()), (64, 7));
    assert_eq!(at_most(&probe7, 4), 7);
    let found = probe(&program(&seven), &seven);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, Code::TooFewValidValues);
    assert_eq!(
        found[0].message,
        "Domain `Rare` produced only 7/64 valid values; at least 8 are needed. Loosen its condition."
    );
    let eight = rare(6);
    assert_eq!(probed(&eight, 0).valid(), 8);
    assert_eq!(at_most(&probed(&eight, 0), 6), 8);
    assert!(probe(&program(&eight), &eight).is_empty());
}

/// AC-SYNTH-67, F4 of D5: a value error in the probe's predicate is a failed draw, never `VL0602`.
#[test]
fn ac_synth_67_a_value_error_is_a_failed_draw_in_the_probe() {
    let src = source(
        "type Inverse = Number where value >= 0 and value <= 3 and value is whole and 6 / value > 1",
        "n: Inverse",
    );
    let inverse = probed(&src, 0);
    let zero = inverse.draws.iter().filter(|d| d.value.as_deref() == Some("0"));
    assert!(zero.clone().count() > 0, "a draw of 0 is what this tests");
    assert!(zero.clone().all(|d| !d.valid));
    assert_eq!(inverse.valid(), 64 - zero.count());
    assert!(probe(&program(&src), &src).is_empty(), "the rest pass, with no VL0602");
}

/// AC-SYNTH-67, F4 and F7 of D5: size is checked before the predicate, which would otherwise run out of budget here
/// (`BUSY`'s condition, `VL0601`). A domain scarce only for size says so; one with a condition to blame doesn't.
#[test]
fn ac_synth_67_size_comes_before_the_predicate_and_is_named_in_a_note() {
    let src = source(
        "type Big = Text where value.length >= 300000 and (every i in range(10000) has every j in range(10000) has i + j >= 0)",
        "b: Big",
    );
    let big = probed(&src, 0);
    assert!(big.draws.iter().all(|d| d.value.is_none() && d.oversized && !d.valid));
    let found = probe(&program(&src), &src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, Code::TooFewValidValues, "{found:?}");
    assert_eq!(
        found[0].message,
        "Domain `Big` produced only 0/64 valid values; at least 8 are needed. Loosen its condition."
    );
    assert_eq!(found[0].notes, ["its values can't fit in a 256 KiB input"]);
    // A scarce domain whose values all fit has no such note.
    let seven = rare(4);
    assert!(probe(&program(&seven), &seven)[0].notes.is_empty());
}

/// F4 of D5 (AC-SYNTH-64): a record whose fields together pass the limit is a failed draw in the probe, its fields' own
/// retries notwithstanding.
#[test]
fn ac_synth_67_a_record_past_the_size_limit_is_a_failed_draw() {
    let src = source(
        "type Long = Text where value.length >= 100000\ntype Trio where value.a != value.b:\n    a: Long\n    b: Long\n    c: Long",
        "t: Trio",
    );
    let trio = probed(&src, 1);
    assert!(
        trio.draws.iter().all(|d| d.value.is_none() && d.oversized),
        "300 000 bytes"
    );
    let src = source(
        "type Long = Text where value.length >= 100000\ntype Duo where value.a != value.b:\n    a: Long\n    b: Long",
        "t: Duo",
    );
    assert_eq!(probed(&src, 1).valid(), 64, "200 000 bytes");
}

/// AC-SYNTH-67: every declared domain is probed, used or not, in type-name order, each from its own declaration hash.
#[test]
fn ac_synth_67_unused_domains_are_probed_in_name_order() {
    let src = format!(
        "language: velme/0.2\n\n{NEVER}\ntype Absent = Number where value >= 1 and value <= 2 and value is whole and (value > 5 or value < 0)\n\ngoal G(n: Number) -> Number:\n    plan: \"p\"\n"
    );
    let found = probe(&program(&src), &src);
    let names: Vec<&str> = found
        .iter()
        .map(|d| d.message.split('`').nth(1).unwrap_or(""))
        .collect();
    assert_eq!(names, ["Absent", "Never"], "{found:?}");
    // Byte order of the names (R-SYNTH-67): `B` sorts before `a`.
    let empty = "Number where value >= 1 and value <= 2 and value is whole and (value > 5 or value < 0)";
    let src = source(&format!("type Aa = {empty}\ntype AB = {empty}"), "n: Number");
    let found = probe(&program(&src), &src);
    let names: Vec<&str> = found
        .iter()
        .map(|d| d.message.split('`').nth(1).unwrap_or(""))
        .collect();
    assert_eq!(names, ["AB", "Aa"], "{found:?}");
}

/// AC-SYNTH-67: the seed is the first eight bytes of the domain's own declaration hash, so a goal's plan, parameter
/// name or other declarations can't move it (the probe takes no model at all); a change to the declaration or to a
/// domain it reaches does.
#[test]
fn ac_synth_67_the_seed_is_declaration_local() {
    let base = probed(&rare(6), 0);
    assert_eq!(base.seed, seed_of(base.declaration_hash));
    assert_eq!(base.draws.len(), 64);
    let other_goal = "language: velme/0.2\n\ntype Rare = Number where value >= 1 and value <= 64 and value is whole and (value <= 6 or value > 1000)\ntype Other = Text one of \"x\"\n\ngoal H(count: Rare, o: Other) -> Text:\n    plan: \"Something else entirely.\"\n";
    assert_eq!(probed(other_goal, 0), base);
    assert_ne!(probed(&rare(7), 0).seed, base.seed);
    let nested = |bound: u32| {
        source(
            &format!(
                "type Part = Number where value >= 0 and value <= {bound}\ntype Pair where value.a <= value.b:\n    a: Part\n    b: Number"
            ),
            "p: Pair",
        )
    };
    assert_ne!(probed(&nested(5), 1).seed, probed(&nested(6), 1).seed);
}

/// AC-SYNTH-67: a record domain's probe draws its nested field domains with their retries, and evaluates only the
/// record's own condition once per draw; a child that never succeeds fails every draw.
#[test]
fn ac_synth_67_nested_domains_in_the_probe() {
    let two = "type Two = Number where value >= 1 and value <= 6 and value is whole and (value == 2 or value == 5)";
    let src = source(
        &format!("{two}\ntype Pair where value.a <= value.b:\n    a: Two\n    b: Number"),
        "p: Pair",
    );
    let pair = probed(&src, 1);
    for draw in &pair.draws {
        let value = draw
            .value
            .as_deref()
            .expect("the nested draw succeeds within its retries");
        assert!(
            value.starts_with(r#"{"a":2,"#) || value.starts_with(r#"{"a":5,"#),
            "{value}"
        );
    }
    let never = source(
        &format!("{NEVER}\ntype Wrapped where value.n >= 0:\n    n: Never"),
        "w: Wrapped",
    );
    let wrapped = probed(&never, 1);
    assert!(wrapped.draws.iter().all(|d| d.value.is_none() && !d.valid));
}

/// AC-SYNTH-70: a whole interval of more than 2⁶⁴ points draws `lo + below(2⁶⁴)`, which is the stream output itself; a
/// non-whole one picks a grid with `below(2)`, then an index among the first 2⁶⁴ points from its lower index.
#[test]
fn ac_synth_70_intervals_over_2_64_points() {
    let whole = probed(
        &source(
            "type D = Number where value >= 0 and value <= 10000000000000000000000000 and value is whole",
            "d: D",
        ),
        0,
    );
    assert_eq!(whole.valid(), 64);
    for (i, draw) in whole.draws.iter().enumerate() {
        let stream = output(whole.seed, i as u64 + 1);
        assert_eq!(draw.value.as_deref(), Some(output(stream, 1).to_string().as_str()));
    }
    assert_eq!(
        draws(&whole, 3),
        ["1385054881844851249", "3641294975668427432", "18427680601230169398"]
    );
    let cents = probed(
        &source(
            "type D = Number where value >= 0 and value <= 10000000000000000000000000",
            "d: D",
        ),
        0,
    );
    assert_eq!(cents.valid(), 64);
    for (i, draw) in cents.draws.iter().enumerate() {
        let stream = output(cents.seed, i as u64 + 1);
        let x = u128::from(output(stream, 2));
        let expected = if output(stream, 1) >> 63 == 0 {
            x.to_string()
        } else {
            let text = format!("{}.{:02}", x / 100, x % 100);
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        };
        assert_eq!(draw.value.as_deref(), Some(expected.as_str()), "draw {i}");
    }
    assert_eq!(draws(&cents, 6)[5], "46145516827704351.91");
}

/// AC-SYNTH-70: with the whole grid empty every draw uses the 2-decimal grid, consuming no `below(2)`; with both empty
/// every draw is the midpoint with no draw at all, and its duplicates all count.
#[test]
fn ac_synth_70_empty_grids() {
    let one = probed(&source("type D = Number where value > 0.1 and value < 0.9", "d: D"), 0);
    for (i, draw) in one.draws.iter().enumerate() {
        let stream = output(one.seed, i as u64 + 1);
        // Indices 11 through 89: 79 points, from the stream's first output.
        let k = 11 + ((u128::from(output(stream, 1)) * 79) >> 64);
        let text = format!("0.{k:02}");
        assert_eq!(draw.value.as_deref(), Some(text.trim_end_matches('0')), "draw {i}");
    }
    assert_eq!(draws(&one, 6), ["0.4", "0.44", "0.14", "0.62", "0.38", "0.63"]);
    let none = probed(&source("type D = Number where value > 0 and value < 0.001", "d: D"), 0);
    assert_eq!(none.valid(), 64);
    assert!(none.draws.iter().all(|d| d.value.as_deref() == Some("0.0005")));
}

/// The scarcity probe needs `Membership` and the program only: no provider, no goal, no contract key (AC-CMP-09).
#[test]
fn ac_cmp_09_the_probe_runs_from_the_program_alone() {
    let src = rare(4);
    let program = program(&src);
    assert_eq!(probe(&program, &src)[0].code, Code::TooFewValidValues);
    // A probe draw fits an input: its JSON decodes as the domain's base.
    for draw in probed(&src, 0).draws {
        let value = draw.value.expect("a value");
        assert!(decode_str(&value, &Type::Number, &program).is_ok());
    }
}

// ---- verification (R-SYNTH-14, R-RUN-28) ----

/// The goal `G` returning its first input.
fn identity(program: &Program) -> ValidIr {
    let mut ir = serde_json::to_value(wired_goal(program, goal_id(program, "G")).expect("IR")).expect("IR");
    let name = program.goals[goal_id(program, "G").0].params[0].name.clone();
    ir["body"] = serde_json::json!({"kind": "input", "name": name});
    valid_ir(program, &ir.to_string())
}

/// A runner that refuses every run with `diagnostic`, as the runtime's entry walk does an input outside its domain.
struct Refuses(Option<Diagnostic>);

impl ChildRunner for Refuses {
    fn run(&self, _: &ValidIr, _: Vec<Value>) -> Result<Invocation, Vec<Diagnostic>> {
        Err(vec![self.0.clone().expect("never run")])
    }

    fn admit(&self, _: &[TestInput]) -> Result<(), Diagnostic> {
        Ok(())
    }
}

/// An internal error from a run keeps its notes in the verdict, so a refused generated input names its path and domain
/// (D5's carried item 2).
#[test]
fn an_internal_error_keeps_the_entry_walks_note() {
    let src = source("type Small = Number where value >= 0 and value <= 3", "n: Small");
    let program = program(&src);
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let note = "an input verification ran, `n`, is outside `Small`: it must be at most 3";
    let refused = Diagnostic::internal_error().with_note(note);
    let goal = goal_id(&program, "G");
    let inputs = goal_inputs(&program, goal, &src, key).expect("inputs");
    match verify(
        &program,
        goal,
        &src,
        &inputs,
        &identity(&program),
        &Refuses(Some(refused)),
    ) {
        Verdict::Internal(d) => {
            assert_eq!(d.code, Code::InternalError);
            assert_eq!(d.notes, [note]);
        }
        other => panic!("{other:?}"),
    }
}

/// Counts its runs of the identity goal.
#[derive(Default)]
struct Counts(std::sync::atomic::AtomicUsize);

impl ChildRunner for Counts {
    fn run(&self, _: &ValidIr, inputs: Vec<Value>) -> Result<Invocation, Vec<Diagnostic>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Invocation {
            result: inputs[0].clone(),
            inputs,
            bindings: Vec::new(),
            fuel: 0,
            memory: 0,
        })
    }

    fn admit(&self, _: &[TestInput]) -> Result<(), Diagnostic> {
        Ok(())
    }
}

/// AC-SYNTH-71, F2 of D5: verification runs on the inputs it is given and generates none of its own, however many
/// candidates a goal's attempts verify: inputs made once before the first request serve them all.
#[test]
fn ac_synth_71_verification_uses_the_inputs_made_once() {
    let src = source("type Small = Number where value >= 0 and value <= 3", "n: Small");
    let program = program(&src);
    let goal = goal_id(&program, "G");
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let ir = identity(&program);
    let made = goal_inputs(&program, goal, &src, key).expect("inputs");
    assert_eq!(
        made,
        goal_inputs(&program, goal, &src, key).expect("inputs"),
        "deterministic"
    );
    let mut one = made.clone();
    one.inputs = vec![TestInput {
        json: r#"{"n":2}"#.to_owned(),
        args: vec![made.inputs[0].args[0].clone()],
    }];
    one.boundary = 1;
    one.pairwise = 0;
    one.random = 0;
    let runner = Counts::default();
    for _ in 0..2 {
        assert!(matches!(
            verify(&program, goal, &src, &one, &ir, &runner),
            Verdict::Accepted(_)
        ));
    }
    assert_eq!(runner.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// ---- exact or absent (R-SYNTH-62, D-167, AC-SYNTH-64) ----

/// AC-SYNTH-64, F5 of D5: bound + 0.01 is no `Number` here, so the exclusive edge is the nearest integer inside, not a
/// rounded value equal to the excluded bound; the domain has members to find, so no `VL0210 0/64`.
#[test]
fn ac_synth_64_an_edge_no_number_holds_is_the_nearest_integer_inside() {
    let src = source(
        "type Huge = Number where value > 800000000000000000000000000",
        "h: Huge",
    );
    let (b, s) = sets(&src, 0);
    assert_eq!(b, json(&["800000000000000000000000001"]));
    assert_eq!(s, b);
    let huge = probed(&src, 0);
    assert!(huge.valid() >= 8, "{}/64", huge.valid());
    // An offset no `Number` holds beside that edge fails its draw, unrounded: two fraction digits don't fit.
    assert!(
        huge.draws.iter().any(|d| d.value.is_none() && !d.oversized && !d.valid),
        "no draw reached the exact-or-absent case"
    );
    for draw in huge.draws.iter().filter(|d| d.valid) {
        let value = draw.value.as_deref().expect("a value");
        assert_ne!(value, "800000000000000000000000000");
    }
    assert!(probe(&program(&src), &src).is_empty());
    let upper = source(
        "type Huge = Number where value < -800000000000000000000000000",
        "h: Huge",
    );
    assert_eq!(sets(&upper, 0).0, json(&["-800000000000000000000000001"]));
    assert_members(&upper, &generate(&upper, KEY), 0);
    assert_members(&src, &generate(&src, KEY), 0);
}

/// AC-SYNTH-64, F6 of D5: a bound checked code holds that won't parse is a bug of ours (`VL0607`), never "no bound",
/// which would generate values outside the domain.
#[test]
fn ac_synth_64_a_bound_that_will_not_parse_is_an_internal_error() {
    let number = source("type D = Number where value >= 1", "d: D");
    let text = source("type D = Text where value.length >= 1", "d: D");
    for (src, field) in [(&number, "lower"), (&text, "min_length")] {
        let mut program = program(src);
        let constraints = &mut program.domains[0].constraints;
        let bound = if field == "lower" {
            constraints.lower.as_mut()
        } else {
            constraints.min_length.as_mut()
        };
        bound.expect("a bound").value = "not a number".to_owned();
        let key: Fingerprint = KEY.parse().expect("a fingerprint");
        let error = test_inputs(&program, goal_id(&program, "G"), src, key, &[]).expect_err("a bug of ours");
        assert_eq!(error.code, Code::InternalError, "{field}");
        let found = probe(&program, src);
        assert_eq!(found.len(), 1, "{field}: {found:?}");
        assert_eq!(found[0].code, Code::InternalError, "{field}");
    }
}

// ---- size before allocation (R-SYNTH-62, AC-SYNTH-64) ----

/// AC-SYNTH-64, the size finding of D5: nesting lists around a long text would copy whole values at each level, and go
/// out of memory, if sizes were checked only once a value exists. Every input stays within 256 KiB; a list too long to
/// fit keeps its place in the set, unbuilt, and the ones that fit are generated.
#[test]
fn ac_synth_64_nested_lists_of_long_text_stay_within_the_input_limit() {
    let long = "type Long = Text where value.length >= 100000";
    let deep = source(long, "xs: List<List<List<List<List<List<Long>>>>>>");
    let inputs = generate(&deep, KEY);
    assert!(inputs.boundary > 0, "the sets that fit still run");
    assert!(inputs.inputs.iter().all(|i| i.json.len() <= 262_144));
    assert!(sets(&deep, 0).0.contains(&"oversized".to_owned()));
    let flat = source(long, "xs: List<Long>");
    let (b, _) = sets(&flat, 0);
    assert_eq!(b.len(), 4);
    assert_eq!(b[2], "oversized", "three long items");
    let inputs = generate(&flat, KEY);
    assert!(inputs.inputs.iter().all(|i| i.json.len() <= 262_144));
    assert!(
        inputs.inputs.iter().any(|i| i.json.len() > 200_000),
        "two long items fit"
    );
}

// ---- totals (R-SYNTH-67, D-167, AC-SYNTH-72) ----

/// What one evaluation of the `n`th domain of `src` spends on the value 1.
fn cost(src: &str, n: usize) -> u64 {
    let program = program(src);
    let membership = Membership::new(&program, src).expect("membership");
    let value = Value::Number(velme_builtins::Number::from(1i64));
    membership.member_within(DomainId(n), &value, u64::MAX).1
}

/// AC-SYNTH-72: a record domain whose nested field never succeeds spends up to 17 evaluations in each of 64 draws, past
/// 64 × `max_fuel`: `VL0601` naming the domain. A system budget would take too long, so each evaluation's `max_fuel`
/// here is twice this predicate's cost, which `probe_domain_within` scales the total by.
#[test]
fn ac_synth_72_a_nested_record_domain_past_the_probes_total_is_vl0601() {
    let src = source(
        &format!("{NEVER}\ntype Wrapped where value.n >= 0:\n    n: Never"),
        "w: Wrapped",
    );
    let program = program(&src);
    let membership = Membership::new(&program, &src).expect("membership");
    let fuel = 2 * cost(&src, 0);
    let error = probe_domain_within(&program, &membership, DomainId(1), fuel).expect_err("past the total");
    assert_eq!(error.code, Code::BudgetExceeded, "{error:?}");
    assert_eq!(
        error.message,
        "Checking generated values against `Never` took too many steps in total."
    );
    assert_eq!(error.help.as_deref(), Some("make its `where` condition cheaper"));
    assert!(
        error.notes.iter().any(|n| n.contains(&(64 * fuel).to_string())),
        "{error:?}"
    );
}

/// AC-SYNTH-72: a domain with one evaluation per draw can't pass the probe's total at any `max_fuel` that lets its
/// evaluations finish, even when they spend all of it: 64 draws of `max_fuel` each are exactly 64 × `max_fuel`.
#[test]
fn ac_synth_72_a_flat_domain_is_unaffected() {
    let src = source(
        "type Flat = Number where value >= 1 and value <= 6 and value + 1 > 0",
        "f: Flat",
    );
    let program = program(&src);
    let membership = Membership::new(&program, &src).expect("membership");
    let fuel = cost(&src, 0);
    let flat = probe_domain_within(&program, &membership, DomainId(0), fuel).expect("within the total");
    assert_eq!(flat.valid(), 64);
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let goal = goal_id(&program, "G");
    let inputs = test_inputs_within(&program, goal, &src, key, &[], 2 * fuel).expect("within the total");
    assert_eq!(inputs.inputs.len(), 64);
    assert_eq!(
        inputs,
        test_inputs(&program, goal, &src, key, &[]).expect("inputs"),
        "no other effect"
    );
}

/// AC-SYNTH-72: a goal whose values are all rejected spends 17 evaluations in each of 256 attempts, past 256 ×
/// `max_fuel`: `VL0601` naming the domain, at twice one evaluation's cost.
#[test]
fn ac_synth_72_a_goal_past_its_total_is_vl0601() {
    let src = source(NEVER, "n: Never");
    let program = program(&src);
    let key: Fingerprint = KEY.parse().expect("a fingerprint");
    let fuel = 2 * cost(&src, 0);
    let goal = goal_id(&program, "G");
    let error = test_inputs_within(&program, goal, &src, key, &[], fuel).expect_err("past the total");
    assert_eq!(error.code, Code::BudgetExceeded, "{error:?}");
    assert_eq!(
        error.message,
        "Checking generated values against `Never` took too many steps in total."
    );
    assert!(
        error.notes.iter().any(|n| n.contains(&(256 * fuel).to_string())),
        "{error:?}"
    );
    // The same goal completes, all its attempts skipped, when one evaluation's budget is large enough for the total.
    let inputs = test_inputs_within(&program, goal, &src, key, &[], 17 * fuel).expect("within the total");
    assert_eq!(stages(&inputs), (0, 0, 0, 0, 256 * 16));
}

// ---- findings of D5's architect recheck ----

/// AC-SYNTH-64, F3: the two bounds' sum is exact only if its coefficient is below 2⁹⁶. Here it isn't, although both bounds
/// and the point between them are `Number`s, so no midpoint exists: both grids are empty, every draw fails, and the
/// domain is scarce rather than given a rounded value.
#[test]
fn ac_synth_64_f3_a_midpoint_whose_sum_no_number_holds_is_absent() {
    let src = source(
        "type Tiny = Number where value > 5.0000000000000000000000000001 and value < 5.0000000000000000000000000003",
        "t: Tiny",
    );
    assert_eq!(sets(&src, 0), (Vec::new(), Vec::new()));
    let tiny = probed(&src, 0);
    assert!(tiny.draws.iter().all(|d| d.value.is_none() && !d.valid && !d.oversized));
    let found = probe(&program(&src), &src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, Code::TooFewValidValues);
    assert_eq!(
        found[0].message,
        "Domain `Tiny` produced only 0/64 valid values; at least 8 are needed. Loosen its condition."
    );
    assert!(found[0].notes.is_empty(), "size is not the reason: {found:?}");
    // Here a rounded sum would give a midpoint inside the interval (`5.0000000000000000000000000005`): the sum being
    // inexact is what makes it absent, and not the interval's test of the result.
    let src = source(
        "type Tiny = Number where value > 5.0000000000000000000000000001 and value < 5.0000000000000000000000000008",
        "t: Tiny",
    );
    assert_eq!(sets(&src, 0), (Vec::new(), Vec::new()));
    assert_eq!(probed(&src, 0).valid(), 0);
}

/// AC-SYNTH-68, F4: R-SYNTH-66's recipe, as text: BLAKE3 of the canonical JSON array of strings, each a line's canonical
/// JSON text, a rejected draw's with `✗` (U+2717) appended inside the string and a value-less failed draw's `-✗`.
#[test]
fn ac_synth_68_f4_the_fingerprint_recipe_as_text() {
    assert_eq!(
        frozen(&[r#"{"x":1}"#, "2\u{2717}", "-\u{2717}"]),
        Fingerprint::of_bytes("[\"{\\\"x\\\":1}\",\"2\u{2717}\",\"-\u{2717}\"]".as_bytes()).to_string()
    );
    let by_hand = |lines: &[String]| {
        let quoted: Vec<String> = lines
            .iter()
            .map(|l| format!("\"{}\"", l.replace('"', "\\\"")))
            .collect();
        Fingerprint::of_bytes(format!("[{}]", quoted.join(",")).as_bytes()).to_string()
    };
    let span = probed(V6, 0);
    let all = draws(&span, 64);
    assert_eq!(all[3], "{\"x\":495,\"y\":-369}\u{2717}");
    assert_eq!(
        by_hand(&all),
        "b3:2622abf4aa792958a07671074087d8856625a1ad7f24e87f071da8c8ca6de64b"
    );
    let lines: Vec<String> = generate(V5, KEY).inputs.iter().map(|i| i.json.clone()).collect();
    assert_eq!(
        by_hand(&lines),
        "b3:d4017ea97ab107c6388fe6041406c41df2b77264e138ee7b11957cc408def8b4"
    );
    // A draw with no value at all.
    let never = source(
        &format!("{NEVER}\ntype Wrapped where value.n >= 0:\n    n: Never"),
        "w: Wrapped",
    );
    let wrapped = probed(&never, 1);
    assert_eq!(draws(&wrapped, 64), vec!["-\u{2717}".to_owned(); 64]);
    assert_eq!(frozen(&draws(&wrapped, 64)), by_hand(&vec!["-\u{2717}".to_owned(); 64]));
}

/// AC-SYNTH-64, F5: an oversized value keeps its position in a list, record or diagonal, and any input holding it is
/// skipped: `xs`'s third boundary value would pair with `-1`, which no boundary input then has, and its fourth still
/// pairs with `0.5`. A domain's own candidate is dropped before filtering instead.
#[test]
fn ac_synth_64_f5_an_oversized_value_keeps_its_position_and_its_input_is_skipped() {
    let src = source(
        "type Long = Text where value.length >= 100000",
        "xs: List<Long>, n: Number",
    );
    let (b, _) = sets(&src, 0);
    assert_eq!(b[2], "oversized");
    let inputs = generate(&src, KEY);
    assert_eq!(inputs.boundary, 5);
    let row: Vec<(usize, String)> = inputs.inputs[..5]
        .iter()
        .map(|i| {
            let Value::List(items) = &i.args[0] else {
                panic!("a list")
            };
            let Value::Number(n) = &i.args[1] else {
                panic!("a number")
            };
            (items.len(), n.to_string())
        })
        .collect();
    assert_eq!(
        row,
        [
            (0, "0".to_owned()),
            (1, "1".to_owned()),
            (2, "0.5".to_owned()),
            (0, "1000000".to_owned()),
            (1, "-1000000".to_owned()),
        ]
    );
    // `"é🙂"` padded to this length is 4 bytes over; `"a"`s and `"Lina"` padded fit exactly or under. Dropped, not kept.
    let edge = source("type Edge = Text where value.length >= 262142", "e: Edge");
    let (b, s) = sets(&edge, 0);
    assert_eq!(b.len(), 2, "{:?}", b.iter().map(|v| v.get(..8)).collect::<Vec<_>>());
    assert!(!b.contains(&"oversized".to_owned()));
    assert_eq!(s.len(), 1);
}
