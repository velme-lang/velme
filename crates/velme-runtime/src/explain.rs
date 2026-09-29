//! Explain mode (`runtime/30` §9): a goal rendered in plain words from its waves, with no LLM and no artifact, so the
//! same program always reads the same.

use velme_sema::hir::{Goal, GoalId, GoalKind, Program};

use crate::plan::waves;

/// The explanation of `goal` (R-RUN-22): a line naming it, then its waves — "First:" / "Then:" for a wave of one
/// binding, "At the same time:" with a line each for several — and the tail. `None` if `goal` isn't in `program`.
pub fn explain(program: &Program, goal: GoalId) -> Option<String> {
    let target = program.goals.get(goal.0)?;
    let mut out = format!("{}\n", target.name);
    let plan = target.plan.as_deref().map(first_sentence).filter(|s| !s.is_empty());
    if target.bindings.is_empty() {
        match plan {
            Some(plan) => out.push_str(&format!("This goal calls no others: {plan}\n")),
            None => out.push_str("This goal calls no others.\n"),
        }
        return Some(out);
    }
    for (n, wave) in waves(target).iter().enumerate() {
        let lines: Vec<String> = wave.iter().filter_map(|i| binding_line(program, target, *i)).collect();
        // A binding that isn't in the program is a bug of ours, not an empty group.
        if lines.len() != wave.len() {
            return None;
        }
        match lines.as_slice() {
            [one] => out.push_str(&format!("{} {one}\n", if n == 0 { "First:" } else { "Then:" })),
            many => {
                out.push_str("At the same time:\n");
                for line in many {
                    out.push_str(&format!("  - {line}\n"));
                }
            }
        }
    }
    // A wired goal has no tail of its own: its last binding is the answer (D-4).
    match (target.kind, plan) {
        (GoalKind::Wired, _) => out.push_str("The answer is `result`.\n"),
        (_, Some(plan)) => out.push_str(&format!("Finally: {plan}\n")),
        (_, None) => {}
    }
    Some(out)
}

/// The line of binding `index` of `goal`: the callee's name in words, then its plan's first sentence, or ``run `Child` ``
/// if it has none (R-RUN-22).
fn binding_line(program: &Program, goal: &Goal, index: usize) -> Option<String> {
    let callee = program.goals.get(goal.bindings.get(index)?.callee.0)?;
    let what = callee
        .plan
        .as_deref()
        .map(first_sentence)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("run `{}`", callee.name));
    Some(format!("{} — {what}", words(&callee.name)))
}

/// A goal name in words: `CalculateScore` is "Calculate score".
fn words(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        let before = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let after = chars.get(i + 1).copied();
        let boundary = c.is_uppercase()
            && before.is_some_and(|b| {
                b.is_lowercase() || b.is_ascii_digit() || (b.is_uppercase() && after.is_some_and(char::is_lowercase))
            });
        if boundary {
            out.push(' ');
        }
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The first sentence of `plan`, its whitespace collapsed to single spaces: up to the first `.`, `!` or `?` that is
/// followed by a capital letter or the end, so `avg.` and `e.g.` don't end it; or all of it.
fn first_sentence(plan: &str) -> String {
    let text = plan.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, (at, c)) in chars.iter().enumerate() {
        let ends = match (chars.get(k + 1), chars.get(k + 2)) {
            (None, _) => true,
            (Some((_, space)), Some((_, next))) => space.is_whitespace() && next.is_uppercase(),
            (Some(_), None) => false,
        };
        if matches!(c, '.' | '!' | '?') && ends {
            return text.get(..=*at).unwrap_or(&text).to_owned();
        }
    }
    text
}
