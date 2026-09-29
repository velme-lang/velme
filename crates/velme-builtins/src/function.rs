//! The value built-ins of `language/14` §2 in pure Rust, with their fuel costs (D-52), shared by the interpreter and
//! the WASM backend's host imports (D-36), and the ordering half of `sort_by` (§4, D-59).

use velme_diagnostics::Code;

use crate::limits::MAX_LIST_SIZE;
use crate::{Number, Value};

/// Why an operation has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `VL0602 ArithmeticError`: division by zero, overflow, or an argument outside what a built-in accepts
    /// (R-TYP-05, R-BLT-05). `op` names the operation and its operands, completing "tried to …" (`reference/90`).
    Arithmetic {
        /// What was attempted: `divide 1 by 0`.
        op: String,
    },
    /// `VL0606 SizeLimitExceeded`: a list longer than `max_list_size` (R-TYP-24).
    ListTooLong {
        /// The length asked for.
        length: Number,
    },
    /// `VL0607 InternalError`: arguments that don't match the built-in's signature, which the validator rules out.
    Internal,
}

impl Error {
    pub(crate) fn arithmetic(op: String) -> Error {
        Error::Arithmetic { op }
    }

    /// The diagnostic code this error is reported with.
    pub fn code(&self) -> Code {
        match self {
            Error::Arithmetic { .. } => Code::ArithmeticError,
            Error::ListTooLong { .. } => Code::SizeLimitExceeded,
            Error::Internal => Code::InternalError,
        }
    }
}

/// A value built-in's result and what the call costs in Velme fuel: the catalog cost, which already counts the call
/// node's own unit but not its argument nodes (`runtime/30` R-RUN-04, D-52).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The result.
    pub value: Value,
    /// The fuel the call costs.
    pub fuel: u64,
}

/// A value built-in (`language/14` §2); collection primitives (§4) take lambdas, so the backend drives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    /// `length`
    Length,
    /// `is_empty`
    IsEmpty,
    /// `maximum`
    Maximum,
    /// `minimum`
    Minimum,
    /// `sum`
    Sum,
    /// `contains`
    Contains,
    /// `abs`
    Abs,
    /// `floor`
    Floor,
    /// `ceil`
    Ceil,
    /// `round`
    Round,
    /// `clamp`
    Clamp,
    /// `concat`
    Concat,
    /// `to_text`
    ToText,
    /// `range`
    Range,
    /// `random`
    Random,
}

impl Function {
    /// Calls the built-in on `args`, which the validator has checked against its signature.
    pub fn call(self, args: &[Value]) -> Result<Output, Error> {
        use Value::{Boolean, List, Nothing, Text};
        let n = |len: usize| u64::try_from(len).unwrap_or(u64::MAX);
        let (value, fuel) = match (self, args) {
            (Function::Length, [List(items)]) => (Number::from(n(items.len())).into(), 1),
            (Function::Length, [Text(text)]) => (Number::from(n(text.chars().count())).into(), 1),
            (Function::IsEmpty, [value]) => {
                let empty = match value {
                    Nothing => true,
                    List(items) => items.is_empty(),
                    Text(text) => text.is_empty(),
                    _ => false,
                };
                (Boolean(empty), 1)
            }
            (Function::Maximum, [List(items)]) => (optional(numbers(items)?.into_iter().max()), 1 + n(items.len())),
            (Function::Minimum, [List(items)]) => (optional(numbers(items)?.into_iter().min()), 1 + n(items.len())),
            (Function::Sum, [List(items)]) => {
                // Strictly left to right from 0, so a rounding step lands where the program says (R-TYP-04).
                let total = numbers(items)?
                    .into_iter()
                    .try_fold(Number::ZERO, Number::checked_add)?;
                (total.into(), 1 + n(items.len()))
            }
            (Function::Contains, [List(items), wanted]) => {
                let at = items.iter().position(|item| item == wanted);
                let scanned = at.map_or(items.len(), |i| i + 1);
                (Boolean(at.is_some()), 1 + n(scanned))
            }
            (Function::Abs, [Value::Number(x)]) => (x.abs().into(), 1),
            (Function::Floor, [Value::Number(x)]) => (x.floor().into(), 1),
            (Function::Ceil, [Value::Number(x)]) => (x.ceil().into(), 1),
            (Function::Round, [Value::Number(x)]) => (x.round().into(), 1),
            (Function::Clamp, [Value::Number(x), Value::Number(low), Value::Number(high)]) => {
                if low > high {
                    return Err(Error::arithmetic(format!("clamp {x} between {low} and {high}")));
                }
                ((*x).clamp(*low, *high).into(), 1)
            }
            (Function::Concat, [Text(a), Text(b)]) => {
                let joined = format!("{a}{b}");
                let fuel = 1 + blocks(joined.len());
                (Text(joined.into()), fuel)
            }
            (Function::ToText, [Value::Number(x)]) => {
                let text = x.to_string();
                let fuel = 1 + blocks(text.len());
                (Text(text.into()), fuel)
            }
            (Function::Range, [Value::Number(count)]) => {
                let items = range(*count)?;
                let fuel = 1 + n(items.len());
                (Value::list(items), fuel)
            }
            (Function::Random, [Value::Number(seed), Value::Number(index)]) => (random(*seed, *index)?.into(), 1),
            _ => return Err(Error::Internal),
        };
        Ok(Output { value, fuel })
    }
}

/// The numbers of a `List<Number>`.
fn numbers(items: &[Value]) -> Result<Vec<Number>, Error> {
    items
        .iter()
        .map(|item| match item {
            Value::Number(x) => Ok(*x),
            _ => Err(Error::Internal),
        })
        .collect()
}

fn optional(x: Option<Number>) -> Value {
    x.map_or(Value::Nothing, Value::Number)
}

/// ⌈bytes / 64⌉, the size unit of text built-ins (D-52). `concat`'s inputs together are exactly its output.
fn blocks(bytes: usize) -> u64 {
    u64::try_from(bytes.div_ceil(64)).unwrap_or(u64::MAX)
}

/// `[0, 1, …, count - 1]`: `count` integer-valued and `≥ 0`, else `VL0602`; above `max_list_size`, `VL0606`.
fn range(count: Number) -> Result<Vec<Value>, Error> {
    if !count.is_integer() || count < Number::ZERO {
        return Err(Error::arithmetic(format!("make a list of {count} numbers with range")));
    }
    let length = count
        .to_i64()
        .filter(|n| n.unsigned_abs() <= MAX_LIST_SIZE)
        .ok_or(Error::ListTooLong { length: count })?;
    Ok((0..length).map(|i| Number::from(i).into()).collect())
}

/// SplitMix64's increment (R-BLT-04).
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64's output function (R-BLT-04).
fn mix(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `random(seed, index)`: the `(index + 1)`-th SplitMix64 output for `seed`, as an exact number in `[0, 1)` with 18
/// decimal places (R-BLT-04, D-22). A pure function of its arguments: no clock or entropy (INV-4).
fn random(seed: Number, index: Number) -> Result<Number, Error> {
    let s = seed.to_i64().ok_or_else(|| {
        Error::arithmetic(format!(
            "use {seed} as a random seed (a whole number from -2^63 to 2^63 - 1)"
        ))
    })?;
    let i = index.to_i64().filter(|i| *i >= 0).ok_or_else(|| {
        Error::arithmetic(format!(
            "use {index} as a random index (a whole number from 0 to 2^63 - 1)"
        ))
    })?;
    let z = s
        .cast_unsigned()
        .wrapping_add(i.cast_unsigned().wrapping_add(1).wrapping_mul(GOLDEN));
    let r = (u128::from(mix(z)) * 1_000_000_000_000_000_000) >> 64;
    // r < 10^18, so r × 10^-18 is exact.
    Number::from_scaled(r, 18).ok_or(Error::Internal)
}

/// The order `sort_by` puts elements in, given their keys in list order: stable, and `descending` keeps equal keys
/// in input order too (R-BLT-11, D-59). Returns element indexes.
pub fn sort_order(keys: &[Number], descending: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (keys.get(a), keys.get(b));
        if descending { b.cmp(&a) } else { a.cmp(&b) }
    });
    order
}

/// What `sort_by` over `n` elements costs apart from its key lambda's body: `1 + n·⌈log2(n+1)⌉`, plus 1 per key
/// evaluation (`language/14` §4, D-52). A fixed formula, never counted comparisons.
pub fn sort_by_fuel(n: u64) -> u64 {
    let log = u64::from(u64::BITS - n.leading_zeros());
    1u64.saturating_add(n.saturating_mul(log)).saturating_add(n)
}

#[cfg(test)]
mod tests {
    use super::mix;

    #[test]
    fn ac_blt_01_mix_matches_the_reference_vectors() {
        // (seed, index, mix(z)) from R-BLT-06; `random` itself is checked in `tests/builtins.rs`.
        for (seed, index, want) in [
            (0i64, 0u64, 0xe220_a839_7b1d_cdaf_u64),
            (0, 1, 0x6e78_9e6a_a1b9_65f4),
            (42, 0, 0xbdd7_3226_2feb_6e95),
            (42, 7, 0xccf6_35ee_9e9e_2fa4),
            (-1, 0, 0xe4d9_7177_1b65_2c20),
        ] {
            let z = seed
                .cast_unsigned()
                .wrapping_add((index + 1).wrapping_mul(super::GOLDEN));
            assert_eq!(mix(z), want, "random({seed}, {index})");
        }
    }
}
