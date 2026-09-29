//! The locked artifacts a run needs (`runtime/30` §2 GoalRegistry, §4 step 1): a goal's and those of everything it
//! calls, loaded and validated before anything runs, so a missing or corrupt artifact stops the run at the start.

use std::collections::BTreeMap;

use velme_diagnostics::Diagnostic;
use velme_sema::hir::{GoalId, Program};

use crate::lock::Lock;
use crate::locked::{LockedGoal, load};
use crate::store::Store;

/// The locked artifact of a goal and of every goal it calls, directly or not.
#[derive(Debug, Clone)]
pub struct Registry {
    goals: BTreeMap<GoalId, LockedGoal>,
}

impl Registry {
    /// Loads `goal`, declared in the project file `file`, and every goal it calls (R-ART-10, R-ART-16), each once.
    /// Every one that can't be loaded is reported, walking the calls depth first in source order, so what a run
    /// says never depends on timing.
    pub fn load(
        program: &Program,
        goal: GoalId,
        file: &str,
        lock: &Lock,
        store: &Store,
    ) -> Result<Registry, Vec<Diagnostic>> {
        let mut goals = BTreeMap::new();
        let mut failures = Vec::new();
        let mut seen = vec![goal];
        let mut pending = vec![goal];
        while let Some(id) = pending.pop() {
            let Some(target) = program.goals.get(id.0) else {
                failures.push(Diagnostic::internal_error());
                continue;
            };
            match load(program, id, file, lock, store) {
                Ok(locked) => {
                    goals.insert(id, locked);
                }
                Err(error) => failures.push(error.diagnostic(&target.name, target.span)),
            }
            // Pushed in reverse, so the first call of the block is walked next.
            for binding in target.bindings.iter().rev() {
                if !seen.contains(&binding.callee) {
                    seen.push(binding.callee);
                    pending.push(binding.callee);
                }
            }
        }
        if failures.is_empty() {
            Ok(Registry { goals })
        } else {
            Err(failures)
        }
    }

    /// The locked artifact of `goal`, if it was loaded.
    pub fn get(&self, goal: GoalId) -> Option<&LockedGoal> {
        self.goals.get(&goal)
    }
}
