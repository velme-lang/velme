//! The value built-ins of `language/14` §2–§4 and the decimal `Number` of `language/11` §3, at the value level; the
//! interpreter and WASM backends call these same functions (D-36).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use proptest::prelude::*;
use velme_builtins::limits::MAX_LIST_SIZE;
use velme_builtins::{Builtin, CATALOG, Error, Function, Number, Output, Value, equals, sort_by_fuel, sort_order};
use velme_diagnostics::Code;

fn num(text: &str) -> Number {
    Number::parse(text).unwrap_or_else(|| panic!("{text} is a Number"))
}

fn n(text: &str) -> Value {
    Value::Number(num(text))
}

fn nums(texts: &[&str]) -> Value {
    Value::list(texts.iter().map(|t| n(t)).collect())
}

/// Calls the built-in `name` through the catalog, as a backend does.
fn call(name: &str, args: &[Value]) -> Result<Output, Error> {
    let builtin = Builtin::find(name).unwrap_or_else(|| panic!("no built-in {name}"));
    builtin.function.expect("a value built-in").call(args, u64::MAX)
}

fn value(name: &str, args: &[Value]) -> Value {
    call(name, args).unwrap_or_else(|e| panic!("{name}: {e:?}")).value
}

fn code(name: &str, args: &[Value]) -> Code {
    call(name, args).expect_err("fails").code()
}

#[test]
fn every_value_builtin_has_a_function_and_no_primitive_does() {
    for b in CATALOG {
        assert_eq!(b.function.is_some(), b.in_checks, "{}", b.name);
    }
}

#[test]
fn ac_blt_01_random_reference_vectors() {
    for (seed, index, want) in [
        ("0", "0", "0.883310808213642685"),
        ("0", "1", "0.431527997048510052"),
        ("42", "0", "0.741564878771823401"),
        ("42", "7", "0.800631876713503438"),
        ("-1", "0", "0.893942920283184507"),
    ] {
        let got = value("random", &[n(seed), n(index)]);
        assert_eq!(got, n(want), "random({seed}, {index})");
        assert_eq!(call("random", &[n(seed), n(index)]).expect("ok").fuel, 1);
    }
}

#[test]
fn ac_blt_02_random_needs_integer_seed_and_index() {
    assert_eq!(code("random", &[n("1.5"), n("0")]), Code::ArithmeticError);
    assert_eq!(code("random", &[n("1"), n("-1")]), Code::ArithmeticError);
    assert_eq!(code("random", &[n("1"), n("0.5")]), Code::ArithmeticError);
    // A 64-bit integer: -2^63 ≤ x < 2^63 (R-TYP-07).
    assert_eq!(
        code("random", &[n("9223372036854775808"), n("0")]),
        Code::ArithmeticError
    );
    assert_eq!(
        code("random", &[n("0"), n("9223372036854775808")]),
        Code::ArithmeticError
    );
    value("random", &[n("-9223372036854775808"), n("9223372036854775807")]);
}

#[test]
fn ac_blt_03_maximum_minimum_and_sum() {
    assert_eq!(value("maximum", &[nums(&[])]), Value::Nothing);
    assert_eq!(value("minimum", &[nums(&[])]), Value::Nothing);
    assert_eq!(value("sum", &[nums(&[])]), n("0"));
    assert_eq!(value("maximum", &[nums(&["3", "9", "2"])]), n("9"));
    assert_eq!(value("minimum", &[nums(&["3", "-9.5", "2"])]), n("-9.5"));
    assert_eq!(value("sum", &[nums(&["0.1", "0.2"])]), n("0.3"));
    // Overflow on the way is VL0602.
    let big = "79228162514264337593543950335";
    assert_eq!(code("sum", &[nums(&[big, "1"])]), Code::ArithmeticError);
}

#[test]
fn ac_blt_04_round_ties_away_from_zero() {
    for (x, want) in [
        ("2.5", "3"),
        ("-2.5", "-3"),
        ("2.4", "2"),
        ("-2.4", "-2"),
        ("0.5", "1"),
        ("7", "7"),
    ] {
        assert_eq!(value("round", &[n(x)]), n(want), "round({x})");
    }
    for (x, floor, ceil) in [
        ("2.5", "2", "3"),
        ("-2.5", "-3", "-2"),
        ("4", "4", "4"),
        ("-0.1", "-1", "0"),
    ] {
        assert_eq!(value("floor", &[n(x)]), n(floor), "floor({x})");
        assert_eq!(value("ceil", &[n(x)]), n(ceil), "ceil({x})");
    }
    assert_eq!(value("abs", &[n("-2.5")]), n("2.5"));
}

#[test]
fn ac_blt_05_range() {
    assert_eq!(value("range", &[n("3")]), nums(&["0", "1", "2"]));
    assert_eq!(value("range", &[n("0")]), nums(&[]));
    assert_eq!(code("range", &[n("-1")]), Code::ArithmeticError);
    assert_eq!(code("range", &[n("1.5")]), Code::ArithmeticError);
    let limit = MAX_LIST_SIZE.to_string();
    let Value::List(items) = value("range", &[n(&limit)]) else {
        panic!("a list")
    };
    assert_eq!(items.len().to_string(), limit);
    let above = (MAX_LIST_SIZE + 1).to_string();
    assert_eq!(code("range", &[n(&above)]), Code::SizeLimitExceeded);
    assert_eq!(code("range", &[n("1e28")]), Code::SizeLimitExceeded);
}

#[test]
fn ac_blt_06_sort_order_is_stable_both_ways() {
    // AC-BLT-13 too: keys of [Item(k:2), Item(k:1), Item(k:1)].
    let keys = [num("2"), num("1"), num("1")];
    assert_eq!(sort_order(&keys, false), [1, 2, 0]);
    assert_eq!(sort_order(&keys, true), [0, 1, 2]);
    let keys = [num("1"), num("3"), num("1"), num("3"), num("2")];
    assert_eq!(sort_order(&keys, false), [0, 2, 4, 1, 3]);
    assert_eq!(sort_order(&keys, true), [1, 3, 4, 0, 2]);
}

#[test]
fn ac_blt_13_equal_keys_keep_input_order() {
    let keys = [num("2"), num("1.0"), num("1")];
    assert_eq!(sort_order(&keys, false), [1, 2, 0]);
    assert_eq!(sort_order(&keys, true), [0, 1, 2]);
    // 1 + n·⌈log2(n+1)⌉ + one per key (language/14 §4).
    assert_eq!(sort_by_fuel(0), 1);
    assert_eq!(sort_by_fuel(3), 1 + 3 * 2 + 3);
    assert_eq!(sort_by_fuel(4), 1 + 4 * 3 + 4);
}

#[test]
fn ac_blt_09_to_text_renders_plain_decimal() {
    assert_eq!(value("to_text", &[n("820")]), Value::text("820"));
    assert_eq!(value("to_text", &[n("0.1")]), Value::text("0.1"));
    assert_eq!(value("to_text", &[Value::Number(-Number::ZERO)]), Value::text("0"));
    assert_eq!(value("to_text", &[n("-0")]), Value::text("0"));
    assert_eq!(value("to_text", &[n("2.50")]), Value::text("2.5"));
    assert_eq!(value("to_text", &[n("-0.05")]), Value::text("-0.05"));
    assert_eq!(value("to_text", &[n("1e20")]), Value::text("100000000000000000000"));
}

fn player(name: &str, score: &str) -> Value {
    Value::record(
        "Player",
        vec![("name".to_owned(), Value::text(name)), ("score".to_owned(), n(score))],
    )
}

#[test]
fn ac_blt_11_clamp_bounds_and_contains_records() {
    assert_eq!(code("clamp", &[n("5"), n("10"), n("1")]), Code::ArithmeticError);
    assert_eq!(value("clamp", &[n("5"), n("1"), n("10")]), n("5"));
    assert_eq!(value("clamp", &[n("-5"), n("1"), n("10")]), n("1"));
    assert_eq!(value("clamp", &[n("50"), n("1"), n("10")]), n("10"));
    let players = Value::list(vec![player("Ada", "3"), player("Bo", "2.50")]);
    assert_eq!(
        value("contains", &[players.clone(), player("Bo", "2.5")]),
        Value::Boolean(true)
    );
    assert_eq!(value("contains", &[players, player("Bo", "2")]), Value::Boolean(false));
}

#[test]
fn ac_blt_12_fuel_is_size_proportional() {
    let range = call("range", &[n("1000")]).expect("ok");
    assert_eq!(range.fuel, 1 + 1000);
    assert_eq!(call("sum", &[range.value]).expect("ok").fuel, 1 + 1000);
    let hundred = value("range", &[n("100")]);
    assert_eq!(call("contains", &[hundred.clone(), n("4")]).expect("ok").fuel, 1 + 5);
    assert_eq!(call("contains", &[hundred, n("100")]).expect("ok").fuel, 1 + 100);
    // Text built-ins: 1 + ⌈bytes/64⌉ (D-52).
    let a = Value::text(&"a".repeat(40));
    let concat = call("concat", &[a.clone(), a]).expect("ok");
    assert_eq!(concat.fuel, 1 + 2);
    // ⌈bytes/64⌉ of both inputs together (D-52), not of each: pinned so a change is visible.
    let one = Value::text("a");
    assert_eq!(call("concat", &[one.clone(), one]).expect("ok").fuel, 2);
    assert_eq!(call("to_text", &[n("1")]).expect("ok").fuel, 1 + 1);
    assert_eq!(call("is_empty", &[Value::text("héllo")]).expect("ok").fuel, 1);
    // `length` of a text and `==` of texts: ⌈bytes/64⌉ more, nothing more for empty text (D-83).
    assert_eq!(call("length", &[Value::text("héllo")]).expect("ok").fuel, 1 + 1);
    assert_eq!(call("length", &[Value::text("")]).expect("ok").fuel, 1);
    assert_eq!(call("length", &[Value::text(&"a".repeat(65))]).expect("ok").fuel, 1 + 2);
    let long = Value::text(&"a".repeat(65));
    assert_eq!(equals(&long, &long, u64::MAX).expect("ok").fuel, 2);
    assert_eq!(equals(&long, &Value::text("a"), u64::MAX).expect("ok").fuel, 0);
    assert_eq!(
        equals(&Value::text(""), &Value::text(""), u64::MAX).expect("ok").fuel,
        0
    );
    // `contains` costs, per item scanned, what `item == x` costs (D-83): 1, plus the pairs a record visits.
    let players = Value::list(vec![player("Ada", "3"), player("Bo", "2")]);
    let fuel = |wanted: Value| call("contains", &[players.clone(), wanted]).expect("ok").fuel;
    // Ada: 1 + the records + "Ada" vs "Bo" = 3; Bo: 1 + the records + name (1 + 1 block) + score = 5.
    assert_eq!(fuel(player("Bo", "2")), 1 + 3 + 5);
    assert_eq!(fuel(player("Cy", "2")), 1 + 3 + 4);
    // `==` of lists: 1 per pair visited, the lists' own included (D-83).
    let pair = nums(&["1", "2"]);
    assert_eq!(equals(&pair, &pair, u64::MAX).expect("ok").fuel, 3);
    assert_eq!(equals(&pair, &nums(&["1", "2", "3"]), u64::MAX).expect("ok").fuel, 1);
    assert_eq!(equals(&nums(&[]), &nums(&[]), u64::MAX).expect("ok").fuel, 1);
    assert_eq!(equals(&n("1"), &n("1"), u64::MAX).expect("ok").fuel, 0);
}

#[test]
fn ac_blt_12_a_call_stops_at_its_budget() {
    // Past its budget, a call whose cost follows from its arguments stops before the work (D-83).
    let range = |budget: u64| Function::Range.call(&[n("1000")], budget);
    assert_eq!(range(1000), Err(Error::OutOfFuel));
    assert_eq!(range(1001).expect("ok").fuel, 1001);
    // A size error still wins over fuel (R-RUN-04).
    assert!(matches!(
        Function::Range.call(&[n("20000")], 10),
        Err(Error::ListTooLong { .. })
    ));
    let text = Value::text(&"a".repeat(640));
    assert_eq!(
        Function::Concat.call(&[text.clone(), text.clone()], 20),
        Err(Error::OutOfFuel)
    );
    assert_eq!(Function::Length.call(&[text], 10), Err(Error::OutOfFuel));
    // Equality over values that share their parts has far more leaves than bytes: 2^60 here.
    let mut deep = Value::list(vec![n("1")]);
    for _ in 0..60 {
        deep = Value::list(vec![deep.clone(), deep]);
    }
    assert_eq!(equals(&deep, &deep, 1000), Err(Error::OutOfFuel));
    // Empty lists all the way down have no leaves, but each pair visited is still paid for.
    let mut empty = nums(&[]);
    for _ in 0..60 {
        empty = Value::list(vec![empty.clone(), empty]);
    }
    assert_eq!(equals(&empty, &empty, 1000), Err(Error::OutOfFuel));
    assert_eq!(
        Function::Contains.call(&[Value::list(vec![empty.clone()]), empty], 1000),
        Err(Error::OutOfFuel)
    );
    let list = Value::list(vec![deep.clone()]);
    assert_eq!(Function::Contains.call(&[list, deep], 1000), Err(Error::OutOfFuel));
    assert_eq!(Error::OutOfFuel.code(), Code::BudgetExceeded);
}

#[test]
fn length_is_empty_and_concat() {
    assert_eq!(value("length", &[Value::text("héllo")]), n("5"));
    assert_eq!(value("length", &[nums(&["1", "2"])]), n("2"));
    assert_eq!(value("is_empty", &[Value::Nothing]), Value::Boolean(true));
    assert_eq!(value("is_empty", &[Value::text("")]), Value::Boolean(true));
    assert_eq!(value("is_empty", &[nums(&[])]), Value::Boolean(true));
    assert_eq!(value("is_empty", &[n("0")]), Value::Boolean(false));
    assert_eq!(value("is_empty", &[player("Ada", "1")]), Value::Boolean(false));
    assert_eq!(
        value("concat", &[Value::text("ab"), Value::text("c")]),
        Value::text("abc")
    );
    // Arguments outside the signature are a Velme bug, never a panic.
    assert_eq!(code("length", &[n("1")]), Code::InternalError);
    assert_eq!(code("sum", &[Value::list(vec![Value::text("x")])]), Code::InternalError);
}

#[test]
fn ac_typ_06_division_by_zero() {
    let error = num("1").checked_div(Number::ZERO).expect_err("no answer");
    assert_eq!(error.code(), Code::ArithmeticError);
    assert_eq!(
        error,
        Error::Arithmetic {
            op: "divide 1 by 0".to_owned()
        }
    );
}

#[test]
fn ac_typ_07_negative_zero_is_zero() {
    let product = Number::ZERO.checked_mul(num("-1")).expect("fits");
    assert_eq!(product.to_string(), "0");
    assert_eq!(product, Number::ZERO);
    assert_eq!(-Number::ZERO, Number::ZERO);
    assert_eq!(num("-0.000"), Number::ZERO);
    let hash = |x: Number| {
        let mut h = DefaultHasher::new();
        x.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash(product), hash(Number::ZERO));
    assert_eq!(
        hash(num("-0.5").checked_add(num("0.5")).expect("fits")),
        hash(Number::ZERO)
    );
}

#[test]
fn ac_typ_08_integers_render_without_a_fraction() {
    assert_eq!(num("820").to_string(), "820");
    assert_eq!(num("820.000").to_string(), "820");
    assert_eq!(num("8.2e2").to_string(), "820");
    assert_eq!(num("0.5").checked_mul(num("4")).expect("fits").to_string(), "2");
}

#[test]
fn ac_typ_15_exact_decimal_arithmetic() {
    let sum = num("0.1").checked_add(num("0.2")).expect("fits");
    assert_eq!(sum, num("0.3"));
    assert_eq!(num("2.50"), num("2.5"));
    assert_eq!(value("to_text", &[n("2.50")]), Value::text("2.5"));
    let third = num("1").checked_div(num("3")).expect("fits");
    assert_eq!(third.to_string(), "0.3333333333333333333333333333");
    assert_eq!(
        third.checked_mul(num("3")).expect("fits").to_string(),
        "0.9999999999999999999999999999"
    );
    // The largest scale that fits: 29 significant digits when the coefficient stays below 2^96.
    let ten_thirds = num("10").checked_div(num("3")).expect("fits");
    assert_eq!(ten_thirds.to_string(), "3.3333333333333333333333333333");
    assert_eq!(
        num("2").checked_div(num("3")).expect("fits").to_string(),
        "0.6666666666666666666666666667"
    );
    assert_eq!(num("0.1").to_string(), "0.1");
}

#[test]
fn number_rounding_and_overflow_at_the_edges() {
    let max = num("79228162514264337593543950335");
    assert_eq!(
        max.checked_add(num("1")).expect_err("overflow").code(),
        Code::ArithmeticError
    );
    assert_eq!(
        max.checked_mul(num("2")).expect_err("overflow").code(),
        Code::ArithmeticError
    );
    assert_eq!(
        max.checked_div(num("0.5")).expect_err("overflow").code(),
        Code::ArithmeticError
    );
    // max + 0.5 is a tie between max (odd) and 2^96 (even), which doesn't fit.
    assert_eq!(
        max.checked_add(num("0.5")).expect_err("overflow").code(),
        Code::ArithmeticError
    );
    assert_eq!(max.checked_add(num("0.4")).expect("rounds"), max);
    assert_eq!(
        max.checked_sub(num("0.5")).expect("rounds").to_string(),
        "79228162514264337593543950334"
    );
    // Below the smallest scale: 1e-28 × 0.5 is a tie between 0 and 1e-28, and 0 is even.
    let tiny = num("1e-28");
    assert_eq!(tiny.checked_mul(num("0.5")).expect("rounds"), Number::ZERO);
    assert_eq!(tiny.checked_mul(num("1.5")).expect("rounds"), num("2e-28"));
    assert_eq!(tiny.checked_div(num("-2")).expect("rounds"), Number::ZERO);
    // A large integer plus a tiny fraction keeps the scale that fits.
    let sum = num("7e28").checked_add(tiny).expect("rounds");
    assert_eq!(sum, num("7e28"));
    assert_eq!(num("1").checked_div(num("7e28")).expect("fits").to_string(), "0");
    assert_eq!(
        num("1").checked_div(num("3e27")).expect("fits").to_string(),
        "0.0000000000000000000000000003"
    );
    assert_eq!(num("-7").checked_div(num("2")).expect("fits"), num("-3.5"));
}

#[test]
fn number_parse_is_strict_json() {
    for text in ["0", "-0.5", "1e+5", "1E-5", "0e0", "10.25", "-7e2"] {
        assert!(Number::parse(text).is_some(), "{text}");
    }
    for text in [
        "1e+-5", "1.", ".5", "01", "-01", "+1", "1.e5", "-", "", "1e", "--1", "1 ", "0x1", "1_0",
    ] {
        assert_eq!(Number::parse(text), None, "{text}");
    }
}

#[test]
fn arithmetic_errors_name_the_operation() {
    let op = |name: &str, args: &[Value]| match call(name, args) {
        Err(Error::Arithmetic { op }) => op,
        other => panic!("{other:?}"),
    };
    // Each completes "`Goal` tried to {op}, which has no answer." (reference/90).
    assert_eq!(op("range", &[n("-1")]), "make a list of -1 numbers with range");
    assert_eq!(op("clamp", &[n("5"), n("10"), n("1")]), "clamp 5 between 10 and 1");
    assert_eq!(
        op("random", &[n("1.5"), n("0")]),
        "use 1.5 as a random seed (a whole number from -2^63 to 2^63 - 1)"
    );
    assert_eq!(
        op("random", &[n("1"), n("-1")]),
        "use -1 as a random index (a whole number from 0 to 2^63 - 1)"
    );
    assert_eq!(
        op("sum", &[nums(&["79228162514264337593543950335", "1"])]),
        "add 79228162514264337593543950335 and 1"
    );
}

#[test]
fn ordering_is_numeric() {
    let mut xs = [num("2.5"), num("-3"), num("0.10"), num("-0.1"), num("10")];
    xs.sort();
    let texts: Vec<String> = xs.iter().map(ToString::to_string).collect();
    assert_eq!(texts, ["-3", "-0.1", "0.1", "2.5", "10"]);
    assert_eq!(num("1e-28").cmp(&Number::ZERO), Ordering::Greater);
    assert!(num("79228162514264337593543950335") > num("7.9228162514264337593543950335"));
}

// --- R-TYP-04 against an exact reference -------------------------------------------------------------------------

/// A non-negative big integer, little-endian base 2^32: just enough to state R-TYP-04 exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Big(Vec<u32>);

impl Big {
    fn from(n: u128) -> Big {
        let mut limbs = Vec::new();
        let mut n = n;
        while n > 0 {
            limbs.push(n as u32);
            n >>= 32;
        }
        Big(limbs)
    }

    fn pow10(n: u32) -> Big {
        (0..n).fold(Big::from(1), |acc, _| acc.mul(&Big::from(10)))
    }

    fn trim(mut self) -> Big {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }

    fn mul(&self, other: &Big) -> Big {
        let mut out = vec![0u64; self.0.len() + other.0.len() + 1];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in other.0.iter().enumerate() {
                let t = out[i + j] + u64::from(a) * u64::from(b) + carry;
                out[i + j] = t & 0xffff_ffff;
                carry = t >> 32;
            }
            out[i + other.0.len()] += carry;
        }
        Big(out.into_iter().map(|x| x as u32).collect()).trim()
    }

    fn add(&self, other: &Big) -> Big {
        let len = self.0.len().max(other.0.len()) + 1;
        let mut out = Vec::with_capacity(len);
        let mut carry = 0u64;
        for i in 0..len {
            let t = u64::from(*self.0.get(i).unwrap_or(&0)) + u64::from(*other.0.get(i).unwrap_or(&0)) + carry;
            out.push(t as u32);
            carry = t >> 32;
        }
        Big(out).trim()
    }

    /// `|self - other|`.
    fn diff(&self, other: &Big) -> Big {
        let (a, b) = if self >= other { (self, other) } else { (other, self) };
        let mut out = Vec::with_capacity(a.0.len());
        let mut borrow = 0i64;
        for i in 0..a.0.len() {
            let mut t = i64::from(a.0[i]) - i64::from(*b.0.get(i).unwrap_or(&0)) - borrow;
            borrow = i64::from(t < 0);
            if t < 0 {
                t += 1 << 32;
            }
            out.push(t as u32);
        }
        Big(out).trim()
    }

    fn is_odd(&self) -> bool {
        self.0.first().is_some_and(|l| l % 2 == 1)
    }
}

impl Ord for Big {
    fn cmp(&self, other: &Big) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Big) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A number as `(negative, coefficient, scale)`, read back from its rendering.
fn parts(x: Number) -> (bool, u128, u32) {
    let text = x.to_string();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.to_owned()),
        None => (false, text.clone()),
    };
    let scale = digits.split_once('.').map_or(0, |(_, frac)| frac.len() as u32);
    (negative, digits.replace('.', "").parse().expect("digits"), scale)
}

/// Checks `got` against R-TYP-04 for the exact value `±p / q`: rounded half-to-even at the largest scale `≤ 28` whose
/// coefficient is below 2^96, or an error if there is none.
fn check_rounding(negative: bool, p: &Big, q: &Big, got: Result<Number, Error>) -> Result<(), TestCaseError> {
    // round(p·10^t / q) < 2^96  ⇔  2·p·10^t < (2^97 − 1)·q (a tie at 2^96 − ½ rounds to the even 2^96).
    let bound = Big::from((1u128 << 97) - 1).mul(q);
    let two_p = p.mul(&Big::from(2));
    let scale = (0..=28u32).rev().find(|&t| two_p.mul(&Big::pow10(t)) < bound);
    let Some(t) = scale else {
        prop_assert_eq!(got.map_err(|e| e.code()), Err(Code::ArithmeticError));
        return Ok(());
    };
    let got = got.map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
    let (got_negative, c, s) = parts(got);
    prop_assert!(s <= t, "scale {} above {}", s, t);
    let c = Big::from(c).mul(&Big::pow10(t - s));
    // |2·c·q − 2·p·10^t| ≤ q, and a tie goes to the even coefficient.
    let error = c.mul(q).mul(&Big::from(2)).diff(&two_p.mul(&Big::pow10(t)));
    prop_assert!(error <= *q, "not the nearest at scale {}", t);
    if error == *q {
        prop_assert!(!c.is_odd(), "a tie rounded to odd");
    }
    if c != Big(Vec::new()) {
        prop_assert_eq!(got_negative, negative);
    }
    Ok(())
}

fn number_in(coefficient: impl Strategy<Value = u128>) -> impl Strategy<Value = Number> {
    (any::<bool>(), coefficient, 0u32..=28).prop_map(|(negative, c, s)| {
        let sign = if negative { "-" } else { "" };
        num(&format!("{sign}{c}e-{s}"))
    })
}

fn number() -> impl Strategy<Value = Number> {
    number_in(prop_oneof![
        0u128..1000,
        0u128..(1 << 40),
        0u128..(1 << 96),
        ((1u128 << 96) - 1000)..(1 << 96)
    ])
}

/// Small divisors, so quotients often end in an exact half at the last scale that fits.
fn divisor() -> impl Strategy<Value = Number> {
    prop_oneof![
        number(),
        number_in(prop_oneof![
            Just(2u128),
            Just(4),
            Just(8),
            Just(16),
            Just(20),
            1u128..64
        ])
    ]
}

/// `(negative, magnitude, scale)` of the exact sum `a + b`, over 10^scale.
fn exact_sum(a: Number, b: Number) -> (bool, Big, u32) {
    let ((an, ac, ae), (bn, bc, be)) = (parts(a), parts(b));
    let e = ae.max(be);
    let x = Big::from(ac).mul(&Big::pow10(e - ae));
    let y = Big::from(bc).mul(&Big::pow10(e - be));
    if an == bn {
        (an, x.add(&y), e)
    } else if x >= y {
        (an, x.diff(&y), e)
    } else {
        (bn, y.diff(&x), e)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn number_rendering_round_trips(x in number()) {
        prop_assert_eq!(Number::parse(&x.to_string()), Some(x));
    }

    #[test]
    fn number_add_and_sub_follow_r_typ_04(a in number(), b in number()) {
        let (negative, p, e) = exact_sum(a, b);
        check_rounding(negative, &p, &Big::pow10(e), a.checked_add(b))?;
        let (negative, p, e) = exact_sum(a, -b);
        check_rounding(negative, &p, &Big::pow10(e), a.checked_sub(b))?;
    }

    #[test]
    fn number_mul_follows_r_typ_04(a in number(), b in number()) {
        let ((an, ac, ae), (bn, bc, be)) = (parts(a), parts(b));
        check_rounding(an != bn, &Big::from(ac).mul(&Big::from(bc)), &Big::pow10(ae + be), a.checked_mul(b))?;
    }

    #[test]
    fn number_div_follows_r_typ_04(a in number(), b in divisor()) {
        let ((an, ac, ae), (bn, bc, be)) = (parts(a), parts(b));
        if bc == 0 {
            prop_assert_eq!(a.checked_div(b).map_err(|e| e.code()), Err(Code::ArithmeticError));
        } else {
            let p = Big::from(ac).mul(&Big::pow10(be));
            let q = Big::from(bc).mul(&Big::pow10(ae));
            check_rounding(an != bn, &p, &q, a.checked_div(b))?;
        }
    }

    #[test]
    fn number_order_is_numeric(a in number(), b in number()) {
        let (negative, p, _) = exact_sum(a, -b);
        let want = if p == Big(Vec::new()) {
            Ordering::Equal
        } else if negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
        prop_assert_eq!(a.cmp(&b), want);
        prop_assert_eq!(a == b, want == Ordering::Equal);
    }
}
