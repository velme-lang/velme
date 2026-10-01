//! The `contract_key` of every goal of the seeded generator's 100-goal program (`delivery/51` §6, D-130, D-131).
//! Report-only: the target is asserted by an ignored `ac_qa_07_*` test.
#![allow(missing_docs, clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use velme_ir::contract_key;
use velme_sema::hir::GoalId;
use velme_test_support::program;
use velme_test_support::workload::goals;

fn fingerprint(c: &mut Criterion) {
    let program = program(&goals(1, 100));
    c.bench_function("fingerprint, 100 goals", |b| {
        b.iter(|| {
            for goal in 0..program.goals.len() {
                black_box(contract_key(&program, GoalId(goal)).expect("a contract key"));
            }
        });
    });
}

criterion_group!(benches, fingerprint);
criterion_main!(benches);
