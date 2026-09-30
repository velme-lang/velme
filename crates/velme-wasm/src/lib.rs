//! The Velme WASM backend (`runtime/31`): a verified leaf goal body as a core WebAssembly module, emitted with
//! `wasm-encoder`, validated with `wasmparser` and run in Wasmtime with no ambient capability. It is an optimization
//! backend (P-4): the interpreter defines what IR means, and a module must give the same value, the same failure and
//! the same fuel and memory (INV-3).

// The one `unsafe` is loading a compiled module from the cache, in `sandbox` (CC-API-04).
#![deny(unsafe_code)]

pub mod abi;
mod cache;
mod code;
mod codec;
mod data;
mod emit;
mod runtime;
mod sandbox;
mod ty;
mod validate;

// `clippy.toml` allows these in `#[test]` bodies only; the helpers of the tests are test code too.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod sandbox_tests;
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests;

use velme_ir::ValidIr;

pub use cache::{CacheDir, Misplaced, cache_off_note};
pub use sandbox::{
    Backstop, FUEL_ALLOWANCE, FUEL_FACTOR, LoadError, MEMORY_FACTOR, Program, Run, Sandbox, UNIT_INSTRUCTIONS,
    WasmtimeFuel, backstop_fuel,
};

/// An emitted module: a pure function of its IR, with no limit baked in (R-SBX-03).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    bytes: Vec<u8>,
    data_bytes: u32,
    /// The layouts the module reads its inputs and leaves its output in.
    signature: ty::Signature,
    literals: Literals,
}

/// What the literals of a module hold, which no run is charged for (D-83): with the inputs and `max_memory`, the
/// bound on what the host reads back of a result (R-SBX-11, D-120). Counted from the literals' values, so it does not
/// change with how they are laid out in the data segment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Literals {
    /// The sum of their logical sizes (`runtime/30` §7.1).
    pub(crate) bytes: u64,
    /// The items of their lists of `Nothing`, which have no size.
    pub(crate) sizeless: u64,
}

impl Module {
    /// The module's bytes, which have passed validation (R-SBX-07).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes its literal data segment takes, which follows the scratch stack of [`abi::SCRATCH_BYTES`]: with
    /// the inputs' bytes, the two figures `StoreLimits` adds to `max_memory` (R-SBX-19, `runtime/31` §6, D-90).
    pub fn data_bytes(&self) -> u32 {
        self.data_bytes
    }
}

/// Why there is no module for a goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitError {
    /// The emitter has no code for this goal, and says what about it: `auto` runs it on the interpreter, and an
    /// explicit `wasm` is `VL0607` (R-SBX-02, R-SBX-16).
    Declined(&'static str),
    /// The goal is composite: its calls are the host scheduler's, and only leaf goal bodies become modules
    /// (R-SBX-17). A caller passes leaves only, so this is its bug and never a decline of a valid program.
    NotLeaf,
    /// A backend bug: IR the validator should have rejected, or a module that didn't validate. `VL0607` under an
    /// explicit `wasm`; under `auto` the leaf runs on the interpreter, with a `--verbose` note (D-121).
    Internal(String),
}

/// The module of the leaf goal `ir` (R-SBX-01): only validated IR is emitted (INV-1), and the module is validated
/// under the R-SBX-07 feature set before anyone sees it.
pub fn emit(ir: &ValidIr) -> Result<Module, EmitError> {
    let (bytes, data_bytes, signature, literals) = emit::module(ir.goal(), ir.body().node())?;
    validate::validate(&bytes).map_err(EmitError::Internal)?;
    Ok(Module {
        bytes,
        data_bytes,
        signature,
        literals,
    })
}
