//! The one `Number` of every program and backend: an exact decimal (`language/11` §3, D-36, CC-DET-03).

use std::cmp::Ordering;
use std::fmt;
use std::ops::Neg;

use crate::Error;

/// Every coefficient is below 2^96 (R-TYP-04 range).
const COEFFICIENT_LIMIT: u128 = 1 << 96;

/// The largest scale (R-TYP-04).
const MAX_SCALE: u32 = 28;

/// `10^n` for a scale difference `n ≤ 28`.
fn pow10(n: u32) -> u128 {
    10u128.checked_pow(n).unwrap_or(u128::MAX)
}

/// An exact decimal `c × 10^-e` with `|c| < 2^96` and `0 ≤ e ≤ 28` (R-TYP-04).
///
/// A value has one representation: no trailing fractional zeros and no `-0` (R-TYP-06), so the derived equality and
/// hash are numeric (R-TYP-08: `2.50 == 2.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Number {
    coefficient: i128,
    scale: u32,
}

impl Number {
    /// `0`.
    pub const ZERO: Number = Number {
        coefficient: 0,
        scale: 0,
    };

    /// `1`.
    pub const ONE: Number = Number {
        coefficient: 1,
        scale: 0,
    };

    /// The number `±magnitude × 10^-scale`, if it is one; normalized.
    fn new(negative: bool, magnitude: u128, scale: u32) -> Option<Number> {
        if magnitude >= COEFFICIENT_LIMIT || scale > MAX_SCALE {
            return None;
        }
        let (mut magnitude, mut scale) = (magnitude, scale);
        while scale > 0 && magnitude % 10 == 0 {
            magnitude /= 10;
            scale -= 1;
        }
        if magnitude == 0 {
            return Some(Number::ZERO);
        }
        // Below 2^96, so the cast is exact.
        let coefficient = magnitude as i128;
        Some(Number {
            coefficient: if negative { -coefficient } else { coefficient },
            scale,
        })
    }

    /// The number `magnitude × 10^-scale`, if it is one.
    pub(crate) fn from_scaled(magnitude: u128, scale: u32) -> Option<Number> {
        Number::new(false, magnitude, scale)
    }

    fn is_negative(self) -> bool {
        self.coefficient < 0
    }

    fn magnitude(self) -> u128 {
        self.coefficient.unsigned_abs()
    }

    /// The number the JSON number `text` spells, read exactly from its digits (exponents allowed); `None` if no
    /// `Number` holds it exactly (`language/11` §10, AC-TYP-16). `-0` is `0`.
    pub fn parse(text: &str) -> Option<Number> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (mantissa, exponent) = match rest.find(['e', 'E']) {
            Some(at) => (rest.get(..at)?, Some(rest.get(at + 1..)?)),
            None => (rest, None),
        };
        let (int, frac) = match mantissa.split_once('.') {
            Some((int, frac)) => (int, Some(frac)),
            None => (mantissa, None),
        };
        // JSON's grammar (RFC 8259 §6): no leading zeros, digits on both sides of `.`, a sign only before the
        // exponent's digits.
        let is_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !is_digits(int) || (int.len() > 1 && int.starts_with('0')) || frac.is_some_and(|f| !is_digits(f)) {
            return None;
        }
        let exponent = match exponent {
            Some(e) if is_digits(e.strip_prefix(['+', '-']).unwrap_or(e)) => e,
            Some(_) => return None,
            None => "0",
        };
        let frac = frac.unwrap_or("");
        let digits = format!("{int}{frac}");
        let significant = digits.trim_start_matches('0');
        let coefficient = significant.trim_end_matches('0');
        if coefficient.is_empty() {
            // Zero in any spelling, `-0` included, is `0` (R-TYP-06).
            return Some(Number::ZERO);
        }
        // Value = coefficient × 10^power, with `power` bounded by the text length plus the exponent.
        let exponent: i64 = exponent.parse().ok()?;
        let trailing = i64::try_from(significant.len() - coefficient.len()).ok()?;
        let power = exponent
            .checked_sub(i64::try_from(frac.len()).ok()?)?
            .checked_add(trailing)?;
        // More than 39 digits overflow `u128` and are far above the range anyway.
        let coefficient: u128 = coefficient.parse().ok()?;
        if power >= 0 {
            let zeros = u32::try_from(power).ok().filter(|z| *z <= MAX_SCALE)?;
            Number::new(negative, coefficient.checked_mul(pow10(zeros))?, 0)
        } else {
            Number::new(negative, coefficient, u32::try_from(power.unsigned_abs()).ok()?)
        }
    }

    /// `self + other`, rounded half-to-even to the largest scale that fits (R-TYP-04); `VL0602` if the integer part
    /// doesn't fit (R-TYP-05).
    pub fn checked_add(self, other: Number) -> Result<Number, Error> {
        self.sum(other)
            .ok_or_else(|| Error::arithmetic(format!("add {self} and {other}")))
    }

    /// `self - other`, as [`Number::checked_add`].
    pub fn checked_sub(self, other: Number) -> Result<Number, Error> {
        self.sum(-other)
            .ok_or_else(|| Error::arithmetic(format!("subtract {other} from {self}")))
    }

    fn sum(self, other: Number) -> Option<Number> {
        let scale = self.scale.max(other.scale);
        let a = Wide::product(self.magnitude(), pow10(scale - self.scale));
        let b = Wide::product(other.magnitude(), pow10(scale - other.scale));
        if self.is_negative() == other.is_negative() {
            return fit(self.is_negative(), a.plus(b), scale);
        }
        match a.cmp(&b) {
            Ordering::Less => fit(other.is_negative(), b.minus(a), scale),
            _ => fit(self.is_negative(), a.minus(b), scale),
        }
    }

    /// `self × other`, as [`Number::checked_add`].
    pub fn checked_mul(self, other: Number) -> Result<Number, Error> {
        fit(
            self.is_negative() != other.is_negative(),
            Wide::product(self.magnitude(), other.magnitude()),
            self.scale + other.scale,
        )
        .ok_or_else(|| Error::arithmetic(format!("multiply {self} by {other}")))
    }

    /// `self / other`, the quotient rounded half-to-even to the largest scale that fits (R-TYP-04); `VL0602` for a
    /// zero divisor or an integer part that doesn't fit (R-TYP-05).
    pub fn checked_div(self, other: Number) -> Result<Number, Error> {
        self.quotient(other)
            .ok_or_else(|| Error::arithmetic(format!("divide {self} by {other}")))
    }

    fn quotient(self, other: Number) -> Option<Number> {
        let (a, b) = (self.magnitude(), other.magnitude());
        if b == 0 {
            return None;
        }
        // At scale `s` the coefficient is round(a × 10^k / b) with k = s + other.scale - self.scale. It grows with
        // `k`, so the digits of a / b are generated until it no longer fits; k = 0 always fits (a < 2^96).
        let shift = i64::from(other.scale) - i64::from(self.scale);
        let first = u32::try_from(shift.max(0)).ok()?;
        let last = u32::try_from(i64::from(MAX_SCALE) + shift).ok()?;
        let (mut quotient, mut remainder) = (a / b, a % b);
        let mut best = None;
        for k in 0..=last {
            if k >= first {
                let half = (remainder * 2).cmp(&b);
                let up = half == Ordering::Greater || (half == Ordering::Equal && quotient % 2 == 1);
                let rounded = quotient + u128::from(up);
                if rounded >= COEFFICIENT_LIMIT {
                    break;
                }
                best = Some((rounded, k));
            }
            if quotient >= COEFFICIENT_LIMIT {
                break;
            }
            let next = remainder * 10;
            quotient = quotient * 10 + next / b;
            remainder = next % b;
        }
        let (coefficient, k) = best?;
        let scale = u32::try_from(i64::from(k) - shift).ok()?;
        Number::new(self.is_negative() != other.is_negative(), coefficient, scale)
    }

    /// `|self|`.
    pub fn abs(self) -> Number {
        Number {
            coefficient: self.coefficient.abs(),
            scale: self.scale,
        }
    }

    /// The magnitude's integer part and fractional digits.
    fn split(self) -> (u128, u128) {
        let unit = pow10(self.scale);
        (self.magnitude() / unit, self.magnitude() % unit)
    }

    /// The integer `±magnitude`; always in range, since it is at most `|self|` rounded up.
    fn integer(negative: bool, magnitude: u128) -> Number {
        Number::new(negative, magnitude, 0).unwrap_or(Number::ZERO)
    }

    /// The largest integer `≤ self`.
    pub fn floor(self) -> Number {
        let (int, frac) = self.split();
        let up = self.is_negative() && frac != 0;
        Number::integer(self.is_negative(), int + u128::from(up))
    }

    /// The smallest integer `≥ self`.
    pub fn ceil(self) -> Number {
        let (int, frac) = self.split();
        let up = !self.is_negative() && frac != 0;
        Number::integer(self.is_negative(), int + u128::from(up))
    }

    /// The nearest integer, ties away from zero (`language/14` §2: `2.5 → 3`, `-2.5 → -3`).
    pub fn round(self) -> Number {
        let (int, frac) = self.split();
        let up = frac * 2 >= pow10(self.scale);
        Number::integer(self.is_negative(), int + u128::from(up))
    }

    /// Whether `self` has no fractional part (R-TYP-07).
    pub fn is_integer(self) -> bool {
        self.scale == 0
    }

    /// `self` as a 64-bit integer: integer-valued and `-2^63 ≤ self < 2^63` (R-TYP-07).
    pub fn to_i64(self) -> Option<i64> {
        if self.is_integer() {
            i64::try_from(self.coefficient).ok()
        } else {
            None
        }
    }

    /// `self` as the two `i64` of a WASM `Number` slot, `(lo, hi)` (`runtime/31` §3, D-113): `lo` is magnitude bits
    /// 0..63; `hi` holds magnitude bits 64..95 in bits 0..31, the scale in bits 32..39 and the sign in bit 63. A
    /// value has one representation, so it has one pair of bits.
    pub fn to_bits(self) -> (u64, u64) {
        let magnitude = self.magnitude();
        // The low 64 bits, then the 32 above them: the magnitude is below 2^96.
        let lo = magnitude as u64;
        let hi = (magnitude >> 64) as u64 | u64::from(self.scale) << SCALE_SHIFT;
        (lo, if self.is_negative() { hi | SIGN_BIT } else { hi })
    }

    /// The number whose [`Number::to_bits`] are `(lo, hi)`. Anything else is not a `Number` slot, and `VL0607` (D-113):
    /// a bit set outside the three fields, a scale above 28, `-0`, or a coefficient with trailing fractional zeros.
    pub fn from_bits(lo: u64, hi: u64) -> Result<Number, Error> {
        let negative = hi & SIGN_BIT != 0;
        let scale = (hi >> SCALE_SHIFT) & 0xFF;
        let high = hi & 0xFFFF_FFFF;
        if hi & !(SIGN_BIT | 0xFF << SCALE_SHIFT | 0xFFFF_FFFF) != 0 {
            return Err(Error::Internal);
        }
        let scale = u32::try_from(scale).map_err(|_| Error::Internal)?;
        let number = Number::new(negative, u128::from(high) << 64 | u128::from(lo), scale).ok_or(Error::Internal)?;
        // `new` normalizes, so bits that aren't the canonical ones come back different.
        if number.to_bits() == (lo, hi) {
            Ok(number)
        } else {
            Err(Error::Internal)
        }
    }
}

/// Where the scale starts in the `hi` half of a `Number` slot (D-113).
const SCALE_SHIFT: u32 = 32;

/// The sign in the `hi` half of a `Number` slot (D-113).
const SIGN_BIT: u64 = 1 << 63;

impl From<i64> for Number {
    fn from(n: i64) -> Number {
        Number {
            coefficient: i128::from(n),
            scale: 0,
        }
    }
}

impl From<u64> for Number {
    fn from(n: u64) -> Number {
        Number {
            coefficient: i128::from(n),
            scale: 0,
        }
    }
}

impl Neg for Number {
    type Output = Number;

    /// `-self`; exact, since the range is symmetric.
    fn neg(self) -> Number {
        Number {
            coefficient: -self.coefficient,
            scale: self.scale,
        }
    }
}

impl Ord for Number {
    /// Numeric order (R-TYP-08).
    fn cmp(&self, other: &Number) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (negative, _) => {
                let scale = self.scale.max(other.scale);
                let a = Wide::product(self.magnitude(), pow10(scale - self.scale));
                let b = Wide::product(other.magnitude(), pow10(scale - other.scale));
                if negative { b.cmp(&a) } else { a.cmp(&b) }
            }
        }
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Number) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Number {
    /// Plain decimal: no exponent, no trailing fractional zeros, integers without a fraction (R-TYP-08).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.is_negative() { "-" } else { "" };
        let (int, frac) = self.split();
        if self.scale == 0 {
            write!(f, "{sign}{int}")
        } else {
            write!(f, "{sign}{int}.{frac:0width$}", width = self.scale as usize)
        }
    }
}

/// The number `±magnitude × 10^-scale`, rounded half-to-even to the largest scale `≤ 28` whose coefficient is below
/// 2^96 (R-TYP-04); `None` if even the integer part doesn't fit.
fn fit(negative: bool, magnitude: Wide, scale: u32) -> Option<Number> {
    let (mut quotient, mut scale) = (magnitude, scale);
    // The digits dropped so far: the first of them, and whether any after it is non-zero.
    let (mut dropped, mut sticky) = (0u64, false);
    loop {
        if scale <= MAX_SCALE {
            let up = dropped > 5 || (dropped == 5 && (sticky || quotient.is_odd()));
            if let Some(coefficient) = quotient.to_u128().and_then(|q| q.checked_add(u128::from(up)))
                && coefficient < COEFFICIENT_LIMIT
            {
                return Number::new(negative, coefficient, scale);
            }
        }
        scale = scale.checked_sub(1)?;
        sticky |= dropped != 0;
        (quotient, dropped) = quotient.div_rem(10);
    }
}

/// A 256-bit unsigned integer, little-endian 64-bit limbs: wide enough for a product of two coefficients (< 2^192)
/// and for a coefficient aligned to scale 28 (< 2^190).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wide([u64; 4]);

impl Wide {
    fn product(a: u128, b: u128) -> Wide {
        let (a, b) = (limbs(a), limbs(b));
        let mut out = [0u64; 4];
        for (i, x) in a.into_iter().enumerate() {
            let mut carry = 0u128;
            for (j, y) in b.into_iter().enumerate() {
                if let Some(slot) = out.get_mut(i + j) {
                    let t = u128::from(x) * u128::from(y) + u128::from(*slot) + carry;
                    *slot = t as u64;
                    carry = t >> 64;
                }
            }
            if let Some(slot) = out.get_mut(i + 2) {
                *slot = carry as u64;
            }
        }
        Wide(out)
    }

    fn plus(self, other: Wide) -> Wide {
        let mut out = [0u64; 4];
        let mut carry = false;
        for ((slot, x), y) in out.iter_mut().zip(self.0).zip(other.0) {
            let (t, c1) = x.overflowing_add(y);
            let (t, c2) = t.overflowing_add(u64::from(carry));
            *slot = t;
            carry = c1 || c2;
        }
        Wide(out)
    }

    /// `self - other`, for `self ≥ other`.
    fn minus(self, other: Wide) -> Wide {
        let mut out = [0u64; 4];
        let mut borrow = false;
        for ((slot, x), y) in out.iter_mut().zip(self.0).zip(other.0) {
            let (t, b1) = x.overflowing_sub(y);
            let (t, b2) = t.overflowing_sub(u64::from(borrow));
            *slot = t;
            borrow = b1 || b2;
        }
        Wide(out)
    }

    fn div_rem(self, divisor: u64) -> (Wide, u64) {
        let mut out = [0u64; 4];
        let mut remainder = 0u128;
        for (slot, x) in out.iter_mut().zip(self.0).rev() {
            let t = (remainder << 64) | u128::from(x);
            *slot = (t / u128::from(divisor)) as u64;
            remainder = t % u128::from(divisor);
        }
        (Wide(out), remainder as u64)
    }

    fn is_odd(self) -> bool {
        self.0[0] % 2 == 1
    }

    fn to_u128(self) -> Option<u128> {
        let [lo, hi, a, b] = self.0;
        (a == 0 && b == 0).then(|| (u128::from(hi) << 64) | u128::from(lo))
    }
}

impl Ord for Wide {
    fn cmp(&self, other: &Wide) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for Wide {
    fn partial_cmp(&self, other: &Wide) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn limbs(n: u128) -> [u64; 2] {
    [n as u64, (n >> 64) as u64]
}
