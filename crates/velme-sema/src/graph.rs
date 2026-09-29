//! Phase 5, call graph (`compiler/20` §3): the goal graph has no cycle (`language/12` R-GOAL-11), and the run tree
//! below each goal stays within its call and depth limits, checked statically (`runtime/30` §7, `VL0605`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use velme_builtins::limits;
use velme_diagnostics::{Code, Diagnostic};
use velme_syntax::ast;

use crate::budget::BudgetKey;
use crate::hir::{Goal, GoalId};

/// Checks the graph of `goals`, declared by `decls`.
pub(crate) fn check_graph(goals: &[Goal], decls: &[&ast::GoalDecl], diags: &mut Vec<Diagnostic>) {
    report_cycles(goals, diags);
    check_limits(goals, decls, diags);
}

/// R-GOAL-11: `VL0304` with the path of the shortest cycle through each goal in source order that isn't already on a
/// reported one, at that goal's call leading around it. A goal on two cycles can be reported for the second.
fn report_cycles(goals: &[Goal], diags: &mut Vec<Diagnostic>) {
    let edges: Vec<Vec<(GoalId, usize)>> = goals
        .iter()
        .map(|g| g.bindings.iter().enumerate().map(|(i, b)| (b.callee, i)).collect())
        .collect();
    let mut reported: BTreeSet<GoalId> = BTreeSet::new();
    for start in (0..goals.len()).map(GoalId) {
        if reported.contains(&start) {
            continue;
        }
        let Some(cycle) = shortest_cycle(start, |g: GoalId| edges.get(g.0).map_or(&[][..], Vec::as_slice)) else {
            continue;
        };
        reported.extend(cycle.iter().map(|&(id, _)| id));
        let name = |id: GoalId| goals.get(id.0).map_or("?", |g| g.name.as_str());
        let path = cycle
            .iter()
            .map(|&(id, _)| name(id))
            .chain(std::iter::once(name(start)))
            .collect::<Vec<_>>()
            .join(" → ");
        let binding = cycle.first().and_then(|&(_, b)| goals.get(start.0)?.bindings.get(b));
        let Some(binding) = binding else {
            continue;
        };
        diags.push(
            Diagnostic::new(
                Code::CallCycle,
                binding.span,
                format!("These goals call each other in a circle: {path}."),
            )
            .with_help("a goal can't use itself, even through other goals; move the shared work into a new goal"),
        );
    }
}

/// The shortest cycle from `start` back to itself, as (node, index of the edge leaving it), found first in edge order.
pub(crate) fn shortest_cycle<'e, Id: Copy + Ord + 'e>(
    start: Id,
    edges: impl Fn(Id) -> &'e [(Id, usize)],
) -> Option<Vec<(Id, usize)>> {
    // Breadth first; `came_from[n]` is the (node, edge) that first reached `n`.
    let mut came_from: BTreeMap<Id, (Id, usize)> = BTreeMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(at) = queue.pop_front() {
        for &(next, edge) in edges(at) {
            if next == start {
                let mut path = vec![(at, edge)];
                let mut cur = at;
                while cur != start {
                    let &(prev, prev_edge) = came_from.get(&cur)?;
                    path.push((prev, prev_edge));
                    cur = prev;
                }
                path.reverse();
                return Some(path);
            }
            if let std::collections::btree_map::Entry::Vacant(slot) = came_from.entry(next) {
                slot.insert((at, edge));
                queue.push_back(next);
            }
        }
    }
    None
}

/// The run tree below a goal: invocations, itself included, and call depth below it (`language/12` §6). `None` when
/// unknown (it reaches a cycle) or already reported below it for exceeding a system cap, so callers aren't reported
/// again (CC-ERR-04).
#[derive(Clone, Copy, Default)]
struct Tree {
    calls: Option<u64>,
    depth: Option<u64>,
}

/// `VL0605` for each goal whose run tree exceeds its effective `calls` or `depth` limit (R-GOAL-20): any goal can be
/// the top of a run, so each is checked against the system caps too (`runtime/30` §7, AC-RUN-06).
fn check_limits(goals: &[Goal], decls: &[&ast::GoalDecl], diags: &mut Vec<Diagnostic>) {
    // Callees before callers; a goal on or above a cycle is never ready and stays unknown.
    let mut waiting: Vec<usize> = goals.iter().map(|g| g.bindings.len()).collect();
    let mut callers: Vec<Vec<GoalId>> = vec![Vec::new(); goals.len()];
    for (i, goal) in goals.iter().enumerate() {
        for binding in &goal.bindings {
            if let Some(c) = callers.get_mut(binding.callee.0) {
                c.push(GoalId(i));
            }
        }
    }
    let mut trees: Vec<Tree> = vec![Tree::default(); goals.len()];
    let mut ready: VecDeque<GoalId> = (0..goals.len())
        .map(GoalId)
        .filter(|g| waiting.get(g.0) == Some(&0))
        .collect();
    while let Some(id) = ready.pop_front() {
        let (Some(goal), Some(decl)) = (goals.get(id.0), decls.get(id.0)) else {
            continue;
        };
        let below = |b: &crate::hir::Binding| trees.get(b.callee.0).copied().unwrap_or_default();
        let calls = goal
            .bindings
            .iter()
            .try_fold(1u64, |n, b| Some(n.saturating_add(below(b).calls?)));
        let depth = goal
            .bindings
            .iter()
            .try_fold(0u64, |n, b| Some(n.max(below(b).depth?.saturating_add(1))));
        let tree = Tree {
            calls: calls.filter(|&n| !exceeds(n, BudgetKey::Calls, goal, decl, diags)),
            depth: depth.filter(|&n| !exceeds(n, BudgetKey::Depth, goal, decl, diags)),
        };
        if let Some(slot) = trees.get_mut(id.0) {
            *slot = tree;
        }
        for &caller in callers.get(id.0).into_iter().flatten() {
            if let Some(w) = waiting.get_mut(caller.0) {
                *w -= 1;
                if *w == 0 {
                    ready.push_back(caller);
                }
            }
        }
    }
}

/// Whether `n` for `key` is over the system cap, reporting `VL0605` if it is over the goal's effective limit.
fn exceeds(n: u64, key: BudgetKey, goal: &Goal, decl: &ast::GoalDecl, diags: &mut Vec<Diagnostic>) -> bool {
    let name = &goal.name;
    let (limit, cap) = match key {
        BudgetKey::Calls => (goal.budget.max_goal_calls, limits::MAX_GOAL_CALLS),
        BudgetKey::Depth => (goal.budget.max_call_depth, limits::MAX_CALL_DEPTH),
        BudgetKey::Cpu | BudgetKey::Memory => return false,
    };
    if n <= limit {
        return false;
    }
    let (headline, count, fewer) = match key {
        BudgetKey::Depth => (
            format!("Goals call each other too deeply while running `{name}`."),
            format!("its calls would nest {n} deep, but its limit is {limit}"),
            "make its chains of calls shorter",
        ),
        _ => (
            format!("Too many goals were called while running `{name}`."),
            format!("it would run {n} goals, counting itself, but its limit is {limit}"),
            "call fewer goals below it — every call counts, even of the same goal",
        ),
    };
    let mut diag = Diagnostic::new(Code::CallLimitExceeded, decl.name.span, headline).with_note(count);
    let declared = decl
        .budget
        .iter()
        .flat_map(|b| &b.items)
        .find(|item| item.name.name == key.as_str());
    diag = match declared {
        Some(item) if limit < cap => diag
            .with_label(item.span, "the limit set here")
            .with_help(format!("raise `{}=` in its budget, or {fewer}", key.as_str())),
        _ => diag.with_help(fewer),
    };
    diags.push(diag);
    n > cap
}
