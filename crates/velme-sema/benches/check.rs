//! Parsing and analysis of the seeded generator's 1 000-line program (`delivery/51` §6, D-130, D-131). Report-only:
//! the targets are asserted by the ignored `ac_cmp_07_*` and `ac_qa_07_*` tests.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use velme_sema::{SourceFile, analyze};
use velme_test_support::workload::source;

fn check(c: &mut Criterion) {
    let file = SourceFile::new("gen.velme", source(1, 1000));
    c.bench_function("parse, 1000 lines", |b| {
        b.iter(|| velme_syntax::parse(black_box(&file)))
    });
    c.bench_function("analyze, 1000 lines", |b| b.iter(|| analyze(black_box(&file))));
}

criterion_group!(benches, check);
criterion_main!(benches);
