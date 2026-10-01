//! The `delivery/51` §6 fingerprinting target on a 100-goal program from the seeded generator (D-130, D-131). Ignored;
//! `cargo xtask gate` runs it in release.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Duration;

use velme_ir::contract_key;
use velme_sema::hir::GoalId;
use velme_test_support::program;
use velme_test_support::workload::{assert_release, best_of, goals};

/// The `contract_key` of every goal of the generated 100-goal program takes under 5 ms, the best of 20 (AC-QA-07,
/// `delivery/51` §6).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_fingerprint_of_a_100_goal_program_is_under_5_ms() {
    assert_release("ac_qa_07");
    let program = program(&goals(1, 100));
    let took = best_of(20, || {
        for goal in 0..program.goals.len() {
            contract_key(&program, GoalId(goal)).expect("a contract key");
        }
    });
    println!("fingerprint, 100 goals: {took:.2?}");
    assert!(took < Duration::from_millis(5), "{took:?}");
}
