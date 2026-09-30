//! A goal's `budget` line (`language/12` §6, R-GOAL-20): each key once, in its unit, a whole number above 0 and no
//! larger than its system cap (`runtime/30` §7).

use std::collections::BTreeSet;

use velme_builtins::limits;
use velme_diagnostics::{Code, Diagnostic, did_you_mean};
use velme_syntax::Unit;
use velme_syntax::ast;

use crate::hir::Budget;

/// A `budget` key (D-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BudgetKey {
    Cpu,
    Memory,
    Calls,
    Depth,
}

impl BudgetKey {
    const ALL: [BudgetKey; 4] = [BudgetKey::Cpu, BudgetKey::Memory, BudgetKey::Calls, BudgetKey::Depth];

    /// The key as written.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            BudgetKey::Cpu => "cpu",
            BudgetKey::Memory => "memory",
            BudgetKey::Calls => "calls",
            BudgetKey::Depth => "depth",
        }
    }

    fn from_name(name: &str) -> Option<BudgetKey> {
        BudgetKey::ALL.into_iter().find(|k| k.as_str() == name)
    }

    /// How many of the key's base amount one `unit` is (a millisecond, a byte or one), or why `unit` is wrong.
    fn scale(self, unit: Option<Unit>) -> Result<u64, &'static str> {
        match (self, unit) {
            (BudgetKey::Cpu, Some(Unit::Ms)) => Ok(1),
            (BudgetKey::Cpu, _) => Err("`cpu` is a time in whole milliseconds, like `cpu=10ms`."),
            (BudgetKey::Memory, Some(Unit::Kb)) => Ok(limits::KIB),
            (BudgetKey::Memory, Some(Unit::Mb)) => Ok(limits::MIB),
            (BudgetKey::Memory, _) => Err("`memory` is a size in `kb` or `mb`, like `memory=4mb`."),
            (BudgetKey::Calls | BudgetKey::Depth, None) => Ok(1),
            (BudgetKey::Calls, Some(_)) => Err("`calls` is a count, so it has no unit, like `calls=16`."),
            (BudgetKey::Depth, Some(_)) => Err("`depth` is a count, so it has no unit, like `depth=4`."),
        }
    }

    /// The system cap in the key's base amount, and as a learner would write it.
    fn cap(self) -> (u64, String) {
        match self {
            BudgetKey::Cpu => {
                let ms = limits::MAX_FUEL / limits::FUEL_PER_MS;
                (ms, format!("{ms}ms"))
            }
            BudgetKey::Memory => (limits::MAX_MEMORY, format!("{}mb", limits::MAX_MEMORY / limits::MIB)),
            BudgetKey::Calls => (limits::MAX_GOAL_CALLS, limits::MAX_GOAL_CALLS.to_string()),
            BudgetKey::Depth => (limits::MAX_CALL_DEPTH, limits::MAX_CALL_DEPTH.to_string()),
        }
    }
}

/// The effective limits for a goal with `line` (R-GOAL-20): `defaults` (the system caps, or a project's `[budget]`), each
/// replaced by a valid item of the line; `VL0308` for each item that isn't.
pub(crate) fn budget(line: Option<&ast::Budget>, defaults: Budget, diags: &mut Vec<Diagnostic>) -> Budget {
    let mut out = defaults;
    let mut seen = BTreeSet::new();
    for item in line.iter().flat_map(|l| &l.items) {
        match limit(item, &mut seen) {
            Ok((key, value)) => match key {
                BudgetKey::Cpu => out.max_fuel = value.saturating_mul(limits::FUEL_PER_MS),
                BudgetKey::Memory => out.max_memory = value,
                BudgetKey::Calls => out.max_goal_calls = value,
                BudgetKey::Depth => out.max_call_depth = value,
            },
            Err(diag) => diags.push(diag),
        }
    }
    out
}

/// The key of `item` and its value in the key's base amount.
fn limit(item: &ast::BudgetItem, seen: &mut BTreeSet<BudgetKey>) -> Result<(BudgetKey, u64), Diagnostic> {
    let invalid = |message: String| Diagnostic::new(Code::InvalidBudget, item.span, message);
    let name = item.name.name.as_str();
    let Some(key) = BudgetKey::from_name(name) else {
        let help = did_you_mean(name, BudgetKey::ALL.map(BudgetKey::as_str))
            .unwrap_or_else(|| "a budget can set `cpu`, `memory`, `calls` and `depth`".to_owned());
        return Err(Diagnostic::new(
            Code::InvalidBudget,
            item.name.span,
            format!("A budget has no limit called `{name}`."),
        )
        .with_help(help));
    };
    if !seen.insert(key) {
        return Err(invalid(format!("`{name}` is already set on this line.")).with_help("keep one of them"));
    }
    let unit = item.unit.as_ref().map(|u| u.unit);
    let scale = key.scale(unit).map_err(|m| invalid(m.to_owned()))?;
    if item.value.text.contains('.') {
        let mut diag = invalid(format!("`{name}` is written as a whole number, without a `.`"));
        if let Some(kb) = fractional_mb_as_kb(key, &item.value.text, unit).filter(|kb| kb * limits::KIB <= key.cap().0)
        {
            diag = diag.with_help(format!("write `{name}={kb}kb`"));
        }
        return Err(diag);
    }
    let Some(count) = whole(&item.value.text) else {
        return Err(invalid(format!("`{name}` must be a whole number.")));
    };
    if count == 0 {
        return Err(invalid(format!("`{name}` must be more than 0.")));
    }
    let value = count.saturating_mul(scale);
    let (cap, cap_text) = key.cap();
    if value > cap {
        return Err(invalid(format!(
            "`budget` can only make limits smaller — `{name}` can be at most `{cap_text}`."
        )));
    }
    Ok((key, value))
}

/// The value of the digits in `text` (`_` allowed, D-78), saturating at `u64::MAX` (it is then above every cap).
fn whole(text: &str) -> Option<u64> {
    let mut digits = text.bytes().filter(|&b| b != b'_').peekable();
    digits.peek()?;
    digits.try_fold(0u64, |n, b| {
        b.is_ascii_digit()
            .then(|| n.saturating_mul(10).saturating_add(u64::from(b - b'0')))
    })
}

/// D-78: `memory=1.5mb` as the whole number of `kb` it equals (`1536`), if it is one.
fn fractional_mb_as_kb(key: BudgetKey, text: &str, unit: Option<Unit>) -> Option<u64> {
    if key != BudgetKey::Memory || unit != Some(Unit::Mb) {
        return None;
    }
    let (int, fraction) = text.split_once('.')?;
    let int = whole(int)?;
    let fraction: String = fraction.chars().filter(|&c| c != '_').collect();
    let scale = 10u64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let fraction = whole(&fraction)?;
    let kb = int
        .checked_mul(scale)?
        .checked_add(fraction)?
        .checked_mul(limits::MIB / limits::KIB)?;
    (kb % scale == 0).then_some(kb / scale)
}
