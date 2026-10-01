//! Validation of a module's bytes with `wasmparser` under the one feature set a Velme module may use
//! (`runtime/31` R-SBX-07).

use wasmparser::{Parser, Payload, Validator, WasmFeatures};

/// Exactly the R-SBX-07 feature set: the MVP without any `f32`/`f64` operation or type (`Number` is decimal, D-36),
/// plus mutable globals, multi-value and bulk memory. Everything else is off: threads, SIMD, reference types,
/// exceptions, tail calls, and the rest of what `wasmparser` knows.
pub(crate) const FEATURES: WasmFeatures = WasmFeatures::MUTABLE_GLOBAL
    .union(WasmFeatures::MULTI_VALUE)
    .union(WasmFeatures::BULK_MEMORY);

/// Whether `bytes` are a module within [`FEATURES`] with no start function; the error says why not. It takes any
/// bytes, so it stays private to the crate (R-SBX-09): the public API validates only what the emitter made.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), String> {
    Validator::new_with_features(FEATURES)
        .validate_all(bytes)
        .map_err(|error| error.to_string())?;
    // Nothing runs when the module is instantiated, so the host's setup before `velme_run` is `velme_alloc` and its
    // own writes (R-SBX-07, D-115). The emitter never makes a start section.
    let start = Parser::new(0)
        .parse_all(bytes)
        .any(|payload| matches!(payload, Ok(Payload::StartSection { .. })));
    if start {
        return Err("the module has a start function".to_owned());
    }
    Ok(())
}
