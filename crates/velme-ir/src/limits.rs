//! The static IR limits of `compiler/21` §7, defined once: the validator checks them (stages 2 and 7), and the prompt
//! builder and tests refer to them by name.

/// Expression nodes per goal, `calls` included.
pub const MAX_NODES: usize = 10_000;

/// Expression nesting depth: the root expression is at depth 1.
pub const MAX_DEPTH: usize = 128;

/// Collection nodes (`map`, `filter`, `find`, `reduce`, `sort_by`, `all`, `any`) nested inside one another's lambdas;
/// one in another's `list` position doesn't count (D-79).
pub const MAX_COLLECTION_NESTING: usize = 4;

/// Items in a `list` node, and elements in any array of a `literal` value.
pub const MAX_LIST_ITEMS: usize = 1_000;

/// UTF-8 bytes in any text of a `literal` value, 64 KiB.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// UTF-8 bytes of the IR document as received, 1 MiB.
pub const MAX_IR_BYTES: usize = 1024 * 1024;
