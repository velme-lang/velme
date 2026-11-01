//! Velme checks: `check` items, `examples` and domain conditions lowered to IR and evaluated on the reference
//! interpreter, with the failure reports of `language/13` §5 (D-80); the membership walk the runtime calls around each
//! body, and the source-literal membership pass (D-164).
#![forbid(unsafe_code)]

mod literals;
mod lower;
mod membership;
mod run;

pub use literals::check_literals;
pub use lower::{Lowered, lower_check, lower_example, result_local};
pub use membership::{DOMAIN_VISIT_FUEL, Membership, Outside, Refused};
pub use run::{Checked, ExampleCase, GoalChecks, Invocation, ItemReport, Part, Stopped};
