//! The budget system caps of `runtime/30` §7 and the `cpu=` conversion, defined once (R-RUN-18, D-77). A goal's
//! `budget` line can only lower them (`language/12` R-GOAL-20).

/// `max_fuel`: Velme fuel per goal invocation.
pub const MAX_FUEL: u64 = 10_000_000;

/// Fuel per millisecond of `cpu=`: a fixed constant, not measured, so budgets stay deterministic (R-RUN-16).
pub const FUEL_PER_MS: u64 = 100_000;

/// `max_memory`: bytes allocated per goal invocation (`runtime/30` §7.1), 64 MiB.
pub const MAX_MEMORY: u64 = 64 * MIB;

/// `max_goal_calls`: goal invocations in a whole run tree, checked statically (`VL0605`).
pub const MAX_GOAL_CALLS: u64 = 128;

/// `max_call_depth`: call depth of a whole run tree, checked statically (`VL0605`).
pub const MAX_CALL_DEPTH: u64 = 32;

/// `max_list_size`: items in any list value.
pub const MAX_LIST_SIZE: u64 = 10_000;

/// `max_output_bytes`: bytes of the canonical JSON of a goal's result, 1 MiB.
pub const MAX_OUTPUT_BYTES: u64 = MIB;

/// `max_wall_clock` in milliseconds: the watchdog's safety net for a whole top-level run (D-51).
pub const MAX_WALL_CLOCK_MS: u64 = 60_000;

/// Bytes in a `kb` of a `memory=` budget (`language/12` §6).
pub const KIB: u64 = 1 << 10;

/// Bytes in an `mb` of a `memory=` budget.
pub const MIB: u64 = 1 << 20;

// R-RUN-24: the watchdog sits well above the whole run's deterministic bound, 1.28 × 10⁹ fuel or about 12.8 s.
const _: () = assert!(MAX_WALL_CLOCK_MS > MAX_GOAL_CALLS * MAX_FUEL / FUEL_PER_MS);
