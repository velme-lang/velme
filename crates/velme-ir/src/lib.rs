//! Velme `ir` crate: see `compiler/20` §2 for its responsibility.
#![forbid(unsafe_code)]

mod json;
pub mod limits;
mod node;
mod validate;

pub use json::{CanonicalError, MAX_JSON_DEPTH, ParseError, from_json_str, to_canonical_string};
pub use node::{BinaryOperator, Call, CallNode, Goal, Lambda, Node, RecordType, ReduceLambda, Type, UnaryOperator};
pub use validate::{Origin, Request, ValidIr, validate};

/// The IR version this crate reads and writes (`compiler/21` R-IR-22).
pub const IR_VERSION: &str = "0.1";

/// The JSON Schema of [`IR_VERSION`], generated from the Rust types (R-IR-20). The committed copy is
/// `crates/velme-ir/schema/ir-0.1.json` (AC-IR-01).
pub fn schema() -> serde_json::Value {
    schemars::schema_for!(Goal).to_value()
}
