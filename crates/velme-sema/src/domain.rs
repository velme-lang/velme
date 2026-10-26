//! Domain declarations (`language/11` §7.1, `language/13` §6.1): closed sets, the split of a condition into simple
//! constraints, and the static checks of source literals against them (R-TYP-29, R-TYP-31, R-TYP-33, R-CHK-14).

use std::collections::BTreeSet;

use velme_builtins::Number;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_syntax::ast::{self, LiteralKind};

use crate::hir::{BinaryOp, Bound, Constraints, DomainRule, DomainType, Expr, ExprKind, Type, UnaryOp};

/// Most members of one closed set (R-TYP-33).
pub(crate) const MAX_MEMBERS: usize = 1_000;

/// Most UTF-8 bytes across one closed set's decoded members (R-TYP-33).
pub(crate) const MAX_MEMBER_BYTES: usize = 64 * 1024;

/// `VL0209` for domain `name` at `span` (R-TYP-29).
pub(crate) fn invalid(name: &str, span: Span, reason: &str) -> Diagnostic {
    Diagnostic::new(
        Code::InvalidDomain,
        span,
        format!("Domain `{name}` is invalid: {reason}."),
    )
}

/// A closed set's members: distinct Text literals on base `Text`, within R-TYP-33's limits. Problems are `VL0209`.
pub(crate) fn members(
    name: &str,
    base: &Type,
    literals: &[ast::Literal],
    span: Span,
    diags: &mut Vec<Diagnostic>,
) -> Vec<String> {
    if !matches!(base, Type::Text | Type::Error) {
        diags.push(
            invalid(name, span, "`one of` lists Text values, so its base must be `Text`")
                .with_help("write `Text one of \"…\", \"…\"`, or use `where` with bounds for numbers"),
        );
        return Vec::new();
    }
    if literals.is_empty() {
        diags.push(
            invalid(name, span, "its closed set has no members").with_help("list at least one value after `one of`"),
        );
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for literal in literals {
        let LiteralKind::Text { value } = &literal.kind else {
            diags.push(
                invalid(name, literal.span, "a closed set's members must be Text")
                    .with_help("write each member in quotes, like `\"LOW\"`"),
            );
            continue;
        };
        if !seen.insert(value.as_str()) {
            diags.push(
                invalid(name, literal.span, &format!("`\"{value}\"` is listed twice"))
                    .with_help("remove the repeated member"),
            );
            continue;
        }
        out.push(value.clone());
    }
    let bytes: usize = out.iter().map(String::len).sum();
    if out.len() > MAX_MEMBERS {
        diags.push(invalid(
            name,
            span,
            &format!("its closed set has {} members, more than {MAX_MEMBERS}", out.len()),
        ));
    } else if bytes > MAX_MEMBER_BYTES {
        diags.push(invalid(
            name,
            span,
            &format!("its members hold {bytes} bytes of text, more than {MAX_MEMBER_BYTES}"),
        ));
    }
    out
}

/// The exact value of a Number literal's text (leading zeros allowed), if it is one.
pub(crate) fn number(text: &str) -> Option<Number> {
    let (sign, digits) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    let trimmed = digits.trim_start_matches('0');
    let digits = if trimmed.is_empty() || trimmed.starts_with('.') {
        format!("0{trimmed}")
    } else {
        trimmed.to_owned()
    };
    Number::parse(&format!("{sign}{digits}"))
}

/// The signed literal `e` stands for: a number, or `-` before one.
fn literal_number(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Number { text } => Some(text.clone()),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Number { text } => Some(format!("-{text}")),
            _ => None,
        },
        _ => None,
    }
}

/// What a simple conjunct constrains.
enum Subject {
    Value,
    Length,
}

fn subject(e: &Expr) -> Option<Subject> {
    match &e.kind {
        ExprKind::Value => Some(Subject::Value),
        ExprKind::Builtin { name: "length", args } if matches!(args.as_slice(), [a] if a.kind == ExprKind::Value) => {
            Some(Subject::Length)
        }
        _ => None,
    }
}

/// The top-level conjuncts of `e`, in source order (R-CHK-14).
fn conjuncts<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match &e.kind {
        ExprKind::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => {
            conjuncts(lhs, out);
            conjuncts(rhs, out);
        }
        _ => out.push(e),
    }
}

/// R-CHK-14: the simple constraints of a well-typed condition on `base`. Duplicates, bad length literals and empty
/// intervals are `VL0209` (R-TYP-29); everything else stays a residual filter.
pub(crate) fn extract(name: &str, base: &Type, condition: &Expr, diags: &mut Vec<Diagnostic>) -> Constraints {
    let mut out = Constraints::default();
    let mut parts = Vec::new();
    conjuncts(condition, &mut parts);
    for part in parts {
        match &part.kind {
            ExprKind::IsWhole {
                operand,
                negated: false,
            } if operand.kind == ExprKind::Value && *base == Type::Number => {
                if out.whole {
                    diags.push(invalid(name, part.span, "it says `value is whole` twice"));
                }
                out.whole = true;
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (subject, literal, op) =
                    match (subject(lhs), literal_number(rhs), subject(rhs), literal_number(lhs)) {
                        (Some(s), Some(n), _, _) => (s, n, *op),
                        (_, _, Some(s), Some(n)) => (s, n, reversed(*op)),
                        _ => continue,
                    };
                let (lower, inclusive) = match op {
                    BinaryOp::Gt => (true, false),
                    BinaryOp::GtEq => (true, true),
                    BinaryOp::Lt => (false, false),
                    BinaryOp::LtEq => (false, true),
                    _ => continue,
                };
                let slot = match (subject, base) {
                    (Subject::Value, Type::Number) => {
                        if lower {
                            &mut out.lower
                        } else {
                            &mut out.upper
                        }
                    }
                    (Subject::Length, Type::Text) => {
                        if !number(&literal).is_some_and(|n| n.is_integer() && n >= Number::from(0_i64)) {
                            diags.push(
                                invalid(name, part.span, &format!("a length can't be compared with `{literal}`"))
                                    .with_help("a length bound is a whole number of 0 or more"),
                            );
                            continue;
                        }
                        if lower {
                            &mut out.min_length
                        } else {
                            &mut out.max_length
                        }
                    }
                    _ => continue,
                };
                if slot.is_some() {
                    let which = if lower { "lower" } else { "upper" };
                    diags.push(
                        invalid(name, part.span, &format!("it has more than one {which} bound"))
                            .with_help("keep one bound in each direction"),
                    );
                    continue;
                }
                *slot = Some(Bound {
                    value: literal,
                    inclusive,
                });
            }
            _ => {}
        }
    }
    if let Some(reason) = empty(&out) {
        diags.push(invalid(name, condition.span, reason).with_help("widen its bounds"));
    }
    out
}

/// `a op n` read as `n op' a`.
fn reversed(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::LtEq => BinaryOp::GtEq,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::GtEq => BinaryOp::LtEq,
        other => other,
    }
}

/// Why the simple interval of `c` holds no value, if it doesn't (R-TYP-29).
fn empty(c: &Constraints) -> Option<&'static str> {
    let bound = |b: &Option<Bound>| b.as_ref().and_then(|b| Some((number(&b.value)?, b.inclusive)));
    let (lo, hi) = (bound(&c.lower), bound(&c.upper));
    if let (Some((lo, li)), Some((hi, hi_inc))) = (lo, hi)
        && (lo > hi || (lo == hi && !(li && hi_inc)))
    {
        return Some("no number lies between its bounds");
    }
    if c.whole {
        let one = Number::from(1_i64);
        let first = lo.and_then(|(lo, inc)| {
            let up = lo.ceil();
            if !inc && up == lo {
                up.checked_add(one).ok()
            } else {
                Some(up)
            }
        });
        let last = hi.and_then(|(hi, inc)| {
            let down = hi.floor();
            if !inc && down == hi {
                down.checked_sub(one).ok()
            } else {
                Some(down)
            }
        });
        if let (Some(first), Some(last)) = (first, last)
            && first > last
        {
            return Some("no whole number lies between its bounds");
        }
    }
    // `Some(None)` is no bound; `None` is a strict bound past `i64`, which no length meets.
    let length = |b: &Option<Bound>, lower: bool| -> Option<Option<i64>> {
        let Some((n, inclusive)) = b
            .as_ref()
            .and_then(|b| Some((number(&b.value)?.to_i64()?, b.inclusive)))
        else {
            return Some(None);
        };
        match (lower, inclusive) {
            (_, true) => Some(n),
            (true, false) => n.checked_add(1),
            (false, false) => n.checked_sub(1),
        }
        .map(Some)
    };
    let fits = || {
        let min = length(&c.min_length, true)?.unwrap_or(0).max(0);
        Some(length(&c.max_length, false)?.is_none_or(|max| max >= min))
    };
    (fits() != Some(true)).then_some("no text length fits its bounds")
}

/// Why the source literal `e` isn't a member of `domain` by its closed set or simple constraints, if it isn't
/// (R-TYP-31); residual conditions are left to `velme-check` (R-CHK-15). `None` for anything that isn't a literal.
pub(crate) fn literal_violation(domain: &DomainType, e: &Expr) -> Option<(String, String)> {
    if let ExprKind::Text { value } = &e.kind {
        let shown = format!("\"{value}\"");
        if let DomainRule::OneOf { members } = &domain.rule {
            return (!members.contains(value)).then(|| (shown, "it isn't one of its members".to_owned()));
        }
        let c = &domain.constraints;
        let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
        let fails = |b: &Option<Bound>, lower: bool| {
            b.as_ref().and_then(|b| {
                let n = number(&b.value)?.to_i64()?;
                let ok = match (lower, b.inclusive) {
                    (true, true) => length >= n,
                    (true, false) => length > n,
                    (false, true) => length <= n,
                    (false, false) => length < n,
                };
                (!ok).then(|| format!("its length must be {} {}", op_word(lower, b.inclusive), b.value))
            })
        };
        return fails(&c.min_length, true)
            .or_else(|| fails(&c.max_length, false))
            .map(|reason| (shown, reason));
    }
    let text = literal_number(e)?;
    let n = number(&text)?;
    let shown = n.to_string();
    let c = &domain.constraints;
    if c.whole && !n.is_integer() {
        return Some((shown, "it must be a whole number".to_owned()));
    }
    let fails = |b: &Option<Bound>, lower: bool| {
        b.as_ref().and_then(|b| {
            let bound = number(&b.value)?;
            let ok = match (lower, b.inclusive) {
                (true, true) => n >= bound,
                (true, false) => n > bound,
                (false, true) => n <= bound,
                (false, false) => n < bound,
            };
            (!ok).then(|| format!("it must be {} {}", op_word(lower, b.inclusive), bound))
        })
    };
    fails(&c.lower, true)
        .or_else(|| fails(&c.upper, false))
        .map(|reason| (shown, reason))
}

fn op_word(lower: bool, inclusive: bool) -> &'static str {
    match (lower, inclusive) {
        (true, true) => "at least",
        (true, false) => "more than",
        (false, true) => "at most",
        (false, false) => "less than",
    }
}
