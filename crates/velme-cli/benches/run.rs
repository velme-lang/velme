//! `velme run --locked` on `find_badge`, CLI start to exit, on the interpreter and on `auto` (`delivery/51` §6, D-130,
//! D-131). Report-only: the target is asserted by an ignored `ac_qa_07_*` test.
#![allow(missing_docs)]

use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use velme_test_support::workload::{find_badge, find_badge_run, velme};

fn run_locked(c: &mut Criterion) {
    let dir = find_badge(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("bench").join("find_badge"));
    let exe = Path::new(env!("CARGO_BIN_EXE_velme"));
    let mut group = c.benchmark_group("velme run --locked, find_badge");
    for backend in ["interp", "auto"] {
        let args = find_badge_run(backend);
        // The first run fills the module cache `auto` reads from (R-SBX-20).
        velme(exe, &dir, &args);
        group.bench_function(backend, |b| b.iter(|| velme(exe, &dir, &args)));
    }
    group.finish();
}

criterion_group!(benches, run_locked);
criterion_main!(benches);
