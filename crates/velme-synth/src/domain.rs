//! The simple constraints of a domain as generation uses them (`compiler/22` R-SYNTH-62, D-164): a Number interval's
//! edges, projection and draws over its whole and 2-decimal grids, and a Text domain's admissible lengths. Exact
//! decimal arithmetic throughout (`language/11` §3): grid indices are `i128`, never floats, and an edge, grid point or
//! offset no `Number` holds exactly is absent, never rounded.

use velme_builtins::Number;
use velme_diagnostics::Diagnostic;
use velme_sema::hir::{Bound, Constraints};

use crate::generate::Stream;

/// The 2-decimal grid's scale, and the step of an exclusive non-whole edge (R-SYNTH-62).
const CENTS: i128 = 100;

/// One end of a Number interval: its bound and whether it is inclusive.
#[derive(Debug, Clone, Copy)]
struct End {
    bound: Number,
    inclusive: bool,
}

impl End {
    /// The end `bound` gives; a bound checked code holds that isn't a `Number` is a bug of ours (`VL0607`).
    fn of(bound: Option<&Bound>) -> Result<Option<End>, Diagnostic> {
        bound
            .map(|b| {
                Number::parse(&b.value)
                    .map(|bound| End {
                        bound,
                        inclusive: b.inclusive,
                    })
                    .ok_or_else(Diagnostic::internal_error)
            })
            .transpose()
    }
}

/// The admissible interval of a Number domain's simple constraints (`language/13` R-CHK-14).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Interval {
    lower: Option<End>,
    upper: Option<End>,
    whole: bool,
}

impl Interval {
    pub(crate) fn of(constraints: &Constraints) -> Result<Interval, Diagnostic> {
        Ok(Interval {
            lower: End::of(constraints.lower.as_ref())?,
            upper: End::of(constraints.upper.as_ref())?,
            whole: constraints.whole,
        })
    }

    /// Whether `n` lies within both bounds (`is whole` aside).
    fn within(&self, n: Number) -> bool {
        let above = self
            .lower
            .is_none_or(|l| if l.inclusive { n >= l.bound } else { n > l.bound });
        let below = self
            .upper
            .is_none_or(|u| if u.inclusive { n <= u.bound } else { n < u.bound });
        above && below
    }

    /// The first and last admissible indices `k` of the grid of `k / scale`, each when its bound is present.
    fn indices(&self, scale: i128) -> (Option<i128>, Option<i128>) {
        let first = self.lower.and_then(|l| {
            let (num, den) = scaled(l.bound, scale)?;
            if l.inclusive {
                ceil_div(num, den)
            } else {
                num.div_euclid(den).checked_add(1)
            }
        });
        let last = self.upper.and_then(|u| {
            let (num, den) = scaled(u.bound, scale)?;
            if u.inclusive {
                Some(num.div_euclid(den))
            } else {
                ceil_div(num, den)?.checked_sub(1)
            }
        });
        (first, last)
    }

    /// `(lo + hi) / 2`, the sum exact and the halving rounded per R-TYP-04: the edge and the draw of an interval too
    /// narrow for its grids; absent if the sum isn't a `Number` or the rounded midpoint isn't inside.
    fn midpoint(&self) -> Option<Number> {
        let (l, u) = (self.lower?, self.upper?);
        exact_sum(l.bound, u.bound)?
            .checked_div(Number::from(2i64))
            .ok()
            .filter(|m| self.within(*m))
    }

    /// The lower edge: the bound if inclusive; the nearest integer inside with `is whole`; else the bound + 0.01, or
    /// the midpoint when that isn't inside the interval, or the nearest integer inside when no `Number` is the bound +
    /// 0.01 (R-SYNTH-62, D-167).
    pub(crate) fn lower_edge(&self) -> Option<Number> {
        let l = self.lower?;
        self.edge(l, self.indices(1).0, 1)
    }

    /// The upper edge, as [`Interval::lower_edge`] the other way.
    pub(crate) fn upper_edge(&self) -> Option<Number> {
        let u = self.upper?;
        self.edge(u, self.indices(1).1, -1)
    }

    /// The edge at `end`, `integer` being the admissible integer nearest it and `toward` the sign of a step inside.
    fn edge(&self, end: End, integer: Option<i128>, toward: i128) -> Option<Number> {
        let integer = || integer.and_then(|k| point(k, 1));
        if self.whole {
            return integer();
        }
        if end.inclusive {
            return Some(end.bound);
        }
        match exact_sum(end.bound, point(toward, CENTS)?) {
            Some(stepped) if self.within(stepped) => Some(stepped),
            Some(_) => self.midpoint(),
            None => integer().filter(|n| self.within(*n)),
        }
    }

    /// `n` projected into the interval: `floor` first with `is whole`, then clamped to the edges (R-SYNTH-62).
    pub(crate) fn project(&self, n: Number) -> Number {
        let mut n = if self.whole { n.floor() } else { n };
        if let Some(lo) = self.lower_edge()
            && n < lo
        {
            n = lo;
        }
        if let Some(hi) = self.upper_edge()
            && n > hi
        {
            n = hi;
        }
        n
    }

    /// A random member of the interval (R-SYNTH-62, D-164), `base` being §7.3's Number draw; `None` if the interval
    /// has no point to draw or the drawn point isn't a `Number`.
    pub(crate) fn draw(&self, stream: &mut Stream, base: impl FnOnce(&mut Stream) -> Number) -> Option<Number> {
        match (self.lower, self.upper) {
            (Some(_), Some(_)) if self.whole => grid_draw(self.grid(1)?, 1, stream),
            (Some(_), Some(_)) => match (self.grid(1), self.grid(CENTS)) {
                // `below(2)` picks a grid only when both have points.
                (Some(whole), Some(cents)) => {
                    if stream.below(2) == 0 {
                        grid_draw(whole, 1, stream)
                    } else {
                        grid_draw(cents, CENTS, stream)
                    }
                }
                (Some(whole), None) => grid_draw(whole, 1, stream),
                (None, Some(cents)) => grid_draw(cents, CENTS, stream),
                (None, None) => self.midpoint(),
            },
            // An edge plus an offset no `Number` holds exactly fails the draw (D-167).
            (Some(_), None) => {
                let offset = self.offset(base(stream));
                exact_sum(self.lower_edge()?, offset)
            }
            (None, Some(_)) => {
                let offset = self.offset(base(stream));
                exact_sum(self.upper_edge()?, -offset)
            }
            (None, None) => {
                let n = base(stream);
                Some(if self.whole { n.floor() } else { n })
            }
        }
    }

    /// A one-sided interval's offset from its edge: the absolute base draw, `floor`ed with `is whole`.
    fn offset(&self, n: Number) -> Number {
        if self.whole { n.abs().floor() } else { n.abs() }
    }

    /// The first and last admissible indices of the grid of `k / scale`, if it has any point.
    fn grid(&self, scale: i128) -> Option<(i128, i128)> {
        match self.indices(scale) {
            (Some(first), Some(last)) if first <= last => Some((first, last)),
            _ => None,
        }
    }
}

/// `lo_s + below(min(hi_s − lo_s + 1, 2⁶⁴))`, divided by `scale` (R-SYNTH-62).
fn grid_draw((first, last): (i128, i128), scale: i128, stream: &mut Stream) -> Option<Number> {
    let count = u128::try_from(last.checked_sub(first)?.checked_add(1)?).ok()?;
    let index = first.checked_add(i128::try_from(stream.points(count)).ok()?)?;
    point(index, scale)
}

/// The number `k / scale`, `scale` being 1 or 100; `None` if no `Number` holds it exactly.
fn point(k: i128, scale: i128) -> Option<Number> {
    decimal(k, if scale == 1 { 0 } else { 2 })
}

/// The number `coefficient × 10^-scale`; `None` if no `Number` holds it exactly.
fn decimal(coefficient: i128, scale: u32) -> Option<Number> {
    Number::parse(&format!("{coefficient}e-{scale}"))
}

/// `n` as `(coefficient, scale)`: `-1.25` is `(-125, 2)`.
fn parts(n: Number) -> Option<(i128, u32)> {
    let text = n.to_string();
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    Some((format!("{int}{frac}").parse().ok()?, u32::try_from(frac.len()).ok()?))
}

/// `a + b` exactly; `None` if no `Number` holds it, where `checked_add` would round (R-TYP-04).
fn exact_sum(a: Number, b: Number) -> Option<Number> {
    let ((ca, sa), (cb, sb)) = (parts(a)?, parts(b)?);
    let scale = sa.max(sb);
    let align = |c: i128, s: u32| c.checked_mul(10i128.checked_pow(scale - s)?);
    decimal(align(ca, sa)?.checked_add(align(cb, sb)?)?, scale)
}

/// `n × scale` as the fraction `num / den`, `den` a power of ten: `-1.25 × 100` is `(-12500, 100)`.
fn scaled(n: Number, scale: i128) -> Option<(i128, i128)> {
    let (coefficient, digits) = parts(n)?;
    Some((coefficient.checked_mul(scale)?, 10i128.checked_pow(digits)?))
}

/// `⌈num / den⌉` for `den > 0`.
fn ceil_div(num: i128, den: i128) -> Option<i128> {
    num.checked_neg()?.div_euclid(den).checked_neg()
}

/// A Text domain's admissible lengths in scalars (R-SYNTH-62): the least and, when bounded, the greatest. Length
/// bounds are non-negative integers (`reference/90` VL0209).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lengths {
    /// The lower edge, when there is a lower bound.
    pub(crate) lower: Option<u128>,
    /// The upper edge, when there is an upper bound; `None` inside when no length is admissible.
    pub(crate) upper: Option<Option<u128>>,
}

impl Lengths {
    /// The lengths `constraints` admit; a length bound that isn't a non-negative integer is a bug of ours (`VL0607`).
    pub(crate) fn of(constraints: &Constraints) -> Result<Lengths, Diagnostic> {
        let parse = |b: &Bound| b.value.parse::<u128>().map_err(|_| Diagnostic::internal_error());
        Ok(Lengths {
            lower: match &constraints.min_length {
                Some(b) if b.inclusive => Some(parse(b)?),
                Some(b) => parse(b)?.checked_add(1),
                None => None,
            },
            upper: match &constraints.max_length {
                Some(b) if b.inclusive => Some(Some(parse(b)?)),
                Some(b) => Some(parse(b)?.checked_sub(1)),
                None => None,
            },
        })
    }

    /// The least admissible length: zero when unbounded below.
    pub(crate) fn least(&self) -> u128 {
        self.lower.unwrap_or(0)
    }

    /// The greatest admissible length, `Some(None)` when unbounded above; `None` if no length is admissible.
    pub(crate) fn greatest(&self) -> Option<Option<u128>> {
        match self.upper {
            None => Some(None),
            Some(Some(hi)) if hi >= self.least() => Some(Some(hi)),
            Some(_) => None,
        }
    }

    /// A random length: `lo + below(min(hi, lo + 16) − lo + 1)` (R-SYNTH-62, D-164); `None` if none is admissible.
    pub(crate) fn draw(&self, stream: &mut Stream) -> Option<u128> {
        let lo = self.least();
        let cap = lo.checked_add(16)?;
        let hi = self.greatest()?.map_or(cap, |hi| hi.min(cap));
        Some(lo + stream.points(hi - lo + 1))
    }
}
