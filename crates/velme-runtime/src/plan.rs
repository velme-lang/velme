//! The call planner (`runtime/30` §2, R-RUN-06): a composite goal's bindings grouped into waves.

use std::collections::BTreeMap;

use velme_sema::hir::Goal;

/// The waves of `goal`, in the order they run: each is the indices into `goal.bindings` of the bindings of one wave, in
/// source order. A binding's wave is 1 + the latest wave among the bindings its arguments use, and inputs are wave 0
/// (`language/12` R-GOAL-14), so the analysis already computed it; everything in one wave is independent of the rest of
/// it. A goal with no calls has none.
pub(crate) fn waves(goal: &Goal) -> Vec<Vec<usize>> {
    let mut by_wave: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, binding) in goal.bindings.iter().enumerate() {
        by_wave.entry(binding.wave).or_default().push(i);
    }
    by_wave.into_values().collect()
}
