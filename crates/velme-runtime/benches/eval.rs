//! The nested `reduce` over `range(1000)` on the interpreter and on WASM, and a run of 128 trivial calls through the
//! scheduler (`delivery/51` §6, D-130, D-131). Report-only: the targets are asserted by ignored `ac_qa_07_*` tests.
#![allow(missing_docs, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::json;
use velme_builtins::execution::Limits;
use velme_builtins::{BUILTINS_VERSION, Number, Value};
use velme_ir::IR_VERSION;
use velme_runtime::{Backend, Lock, Options, Registry, Store, Wasm, eval_leaf, run_goal};
use velme_test_support::workload::{FAN_FILE, fan_out, fan_source, nested_reduce};
use velme_test_support::{goal_id, program, valid_ir};

fn reduce(c: &mut Criterion) {
    let program = program("language: velme/0.1\n\ngoal G() -> Number:\n    plan: \"Count to a million.\"\n");
    let document = json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": "G", "types": {},
        "inputs": [], "output": {"t": "Number"}, "body": nested_reduce(1000)});
    let (goal, ir) = (&program.goals[0], valid_ir(&program, &document.to_string()));
    let wasm = Backend::Wasm(Arc::new(Wasm::new(None)));
    let mut group = c.benchmark_group("nested reduce, range(1000)");
    group.sample_size(10);
    for (name, backend) in [("interp", &Backend::Interp), ("wasm", &wasm)] {
        // The module is compiled before the timing starts (D-131).
        let _ = eval_leaf(backend, goal, &ir, Vec::new(), Limits::SYSTEM, None);
        group.bench_function(name, |b| {
            b.iter(|| {
                eval_leaf(backend, goal, &ir, Vec::new(), Limits::SYSTEM, None)
                    .0
                    .expect("a million")
            });
        });
    }
    group.finish();
}

fn scheduler(c: &mut Criterion) {
    let (dir, program) = fan_out(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("bench").join("fan"));
    let (source, fan) = (fan_source(), goal_id(&program, "Fan"));
    let lock = Lock::read(&dir).expect("lock").expect("a lock");
    let registry = Registry::load(&program, fan, FAN_FILE, &lock, &Store::new(&dir)).expect("registry");
    c.bench_function("scheduler, 128 calls", |b| {
        b.iter(|| {
            let inputs = vec![Value::from(Number::from(1_i64))];
            run_goal(&program, fan, &source, &registry, inputs, Options::default())
        });
    });
}

criterion_group!(benches, reduce, scheduler);
criterion_main!(benches);
