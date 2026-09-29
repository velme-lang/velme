//! Velme `ir` crate: see `compiler/20` §2 for its responsibility.
#![forbid(unsafe_code)]

mod fingerprint;
mod json;
pub mod limits;
mod lower;
mod mapping;
mod node;
mod validate;

pub use fingerprint::{
    Fingerprint, FingerprintError, Synthesis, compatibility, contract_key, execution_id, signature, synthesis_key,
};
pub use json::{CanonicalError, MAX_JSON_DEPTH, ParseError, from_json_str, from_json_str_within, to_canonical_string};
pub use lower::{calls, ir_type};
pub use mapping::{
    DecodeError, DecodeProblem, OutputTooBig, SHOWN_CHARS, SHOWN_ITEMS, SHOWN_TOTAL, decode_str, decode_value,
    display_value, encode_value, json_kind,
};
pub use node::{
    BinaryOperator, Call, CallNode, Goal, Lambda, LiteralValue, Node, RecordType, ReduceLambda, Type, UnaryOperator,
};
pub use validate::{CheckScope, Origin, Request, Trusted, TrustedExpr, ValidIr, validate};

/// The IR version this crate reads and writes (`compiler/21` R-IR-22).
pub const IR_VERSION: &str = "0.1";

/// The JSON Schema of [`IR_VERSION`], generated from the Rust types (R-IR-20). The committed copy is
/// `crates/velme-ir/schema/ir-0.1.json` (AC-IR-01).
pub fn schema() -> serde_json::Value {
    schemars::schema_for!(Goal).to_value()
}
