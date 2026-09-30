//! Validation of a module's bytes with `wasmparser` under the one feature set a Velme module may use
//! (`runtime/31` R-SBX-07).

use wasmparser::{Validator, WasmFeatures};

/// Exactly the R-SBX-07 feature set: the MVP without any `f32`/`f64` operation or type (`Number` is decimal, D-36),
/// plus mutable globals, multi-value and bulk memory. Everything else is off: threads, SIMD, reference types,
/// exceptions, tail calls, and the rest of what `wasmparser` knows.
pub(crate) const FEATURES: WasmFeatures = WasmFeatures::MUTABLE_GLOBAL
    .union(WasmFeatures::MULTI_VALUE)
    .union(WasmFeatures::BULK_MEMORY);

/// Whether `bytes` are a module within [`FEATURES`]; the error says why not. It takes any bytes, so it stays
/// private to the crate (R-SBX-09): the public API validates only what the emitter made.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), String> {
    Validator::new_with_features(FEATURES)
        .validate_all(bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
