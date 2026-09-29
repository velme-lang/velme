//! Velme checks: `check` items and `examples` lowered to IR and evaluated on the reference interpreter, with the
//! failure reports of `language/13` §5 (D-80).
#![forbid(unsafe_code)]

mod lower;
mod run;

pub use lower::{Lowered, lower_check, lower_example, record_types, result_local};
pub use run::{Checked, ExampleCase, GoalChecks, Invocation, ItemReport, Part};
