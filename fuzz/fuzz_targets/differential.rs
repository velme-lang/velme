//! Both backends agree on every generated leaf goal: the same value or the same full diagnostic, fuel and memory, also
//! within limits cut short, and no run near its Wasmtime backstops (`runtime/31` R-SBX-15, D-118). Run with
//! `cargo +nightly fuzz run differential`; `fuzz/corpus/differential/` is replayed by `cargo test` on stable (AC-QA-06).
#![no_main]

use std::sync::{Arc, LazyLock};

use libfuzzer_sys::arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use velme_runtime::Wasm;
use velme_test_support::generate::{check, generate};

/// One backend for the whole run, with no disk cache; it keeps only the modules it compiled last in memory.
static WASM: LazyLock<Arc<Wasm>> = LazyLock::new(|| Arc::new(Wasm::new(None)));

fuzz_target!(|data: &[u8]| {
    let generated = generate(&mut Unstructured::new(data));
    if let Err(mismatch) = check(&WASM, &generated) {
        panic!("the backends differ: {mismatch}");
    }
});
