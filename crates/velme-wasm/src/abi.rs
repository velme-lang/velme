//! The interface of an emitted module and the layout of its memory, shared by the emitter and the host (`runtime/31`
//! §3, §5): the exports, the imports whitelist, the trap reasons and the scratch stack. Defined once (CC-CONST-01).

use velme_builtins::limits::MAX_LIST_SIZE;
use velme_ir::limits::MAX_COLLECTION_NESTING;

use crate::code::V;

/// The export of the linear memory (R-SBX-03).
pub const MEMORY: &str = "memory";

/// The export `velme_alloc(size: i32) -> i32`: uncharged room for the host to write the inputs in (R-SBX-03).
pub const ALLOC: &str = "velme_alloc";

/// The export `velme_run(input_ptr: i32) -> i32`: the goal body, returning the address of the output's slot, which
/// is a record's own and [`RESULT`] for any other type (R-SBX-03).
pub const RUN: &str = "velme_run";

/// The exported mutable `i64` global holding the Velme fuel left, unsigned: the host sets it before `velme_run` and
/// reads it afterwards, also after a trap (R-SBX-03, D-113).
pub const FUEL_LEFT: &str = "velme_fuel_left";

/// The exported mutable `i64` global holding the bytes left to allocate, unsigned (R-SBX-03, D-113).
pub const MEMORY_LEFT: &str = "velme_memory_left";

/// The exported mutable `i32` global holding the [`Reason`] of a trap the module raised itself (R-SBX-03, D-113).
pub const REASON: &str = "velme_reason";

/// The module name of every import (`runtime/31` §5).
pub const IMPORT_MODULE: &str = "velme";

/// Why a module trapped, as `velme_reason` holds it (R-SBX-05, R-SBX-11). A failure an import raises — `VL0602`,
/// `VL0606` — is kept by the host, which has the operands its message names, and leaves the reason at `None`
/// (R-SBX-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The module raised nothing: it finished, an import failed, or the trap is Wasmtime's own.
    None,
    /// `VL0601`: a fuel charge passed `velme_fuel_left`, which is now 0.
    OutOfFuel,
    /// `VL0604`: a memory charge passed `velme_memory_left`, which is now 0.
    OutOfMemory,
    /// `VL0607`: the module met a state its own code rules out, such as a list longer than `max_list_size`.
    Internal,
    /// The host refused to grow the linear memory: the `StoreLimits` backstop (`runtime/31` §6, R-SBX-12).
    GrowRefused,
}

impl Reason {
    /// Every reason, in code order.
    pub const ALL: [Reason; 5] = [
        Reason::None,
        Reason::OutOfFuel,
        Reason::OutOfMemory,
        Reason::Internal,
        Reason::GrowRefused,
    ];

    /// The value of `velme_reason`.
    pub fn code(self) -> i32 {
        match self {
            Reason::None => 0,
            Reason::OutOfFuel => 1,
            Reason::OutOfMemory => 2,
            Reason::Internal => 3,
            Reason::GrowRefused => 4,
        }
    }

    /// The reason `velme_reason` holds, if `code` is one.
    pub fn from_code(code: i32) -> Option<Reason> {
        Reason::ALL.into_iter().find(|reason| reason.code() == code)
    }
}

/// A host import (`runtime/31` §5, D-112): the whitelist is this enum and nothing else. Each does a constant amount
/// of host work, and a `Number` crosses as two `i64`, `lo` then `hi` (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Import {
    /// `num_add: (n, n) -> n`
    NumAdd,
    /// `num_sub: (n, n) -> n`
    NumSub,
    /// `num_mul: (n, n) -> n`
    NumMul,
    /// `num_div: (n, n) -> n`
    NumDiv,
    /// `num_neg: (n) -> n`
    NumNeg,
    /// `num_cmp: (n, n) -> i32`, exactly -1, 0 or 1 as the first is below, equal to or above the second: emitted
    /// code compares the result with those three (D-119).
    NumCmp,
    /// `abs: (n) -> n`
    Abs,
    /// `floor: (n) -> n`
    Floor,
    /// `ceil: (n) -> n`
    Ceil,
    /// `round: (n) -> n`
    Round,
    /// `clamp: (n, n, n) -> n`
    Clamp,
    /// `random: (n, n) -> n`
    Random,
    /// `to_text: (n, ptr: i32) -> i32`: writes the text at `ptr`, at most [`MARSHAL_BYTES`], and returns its length.
    ToText,
    /// `range_len: (n) -> i32`: the length of `range(n)`.
    RangeLen,
}

const N: [V; 2] = [V::I64; 2];
const NN: [V; 4] = [V::I64; 4];
const NNN: [V; 6] = [V::I64; 6];

impl Import {
    /// Every import, in function-index order: a module imports them all, so an index is the same in every module.
    pub const ALL: [Import; 14] = [
        Import::NumAdd,
        Import::NumSub,
        Import::NumMul,
        Import::NumDiv,
        Import::NumNeg,
        Import::NumCmp,
        Import::Abs,
        Import::Floor,
        Import::Ceil,
        Import::Round,
        Import::Clamp,
        Import::Random,
        Import::ToText,
        Import::RangeLen,
    ];

    /// The import's name under [`IMPORT_MODULE`].
    pub fn name(self) -> &'static str {
        match self {
            Import::NumAdd => "num_add",
            Import::NumSub => "num_sub",
            Import::NumMul => "num_mul",
            Import::NumDiv => "num_div",
            Import::NumNeg => "num_neg",
            Import::NumCmp => "num_cmp",
            Import::Abs => "abs",
            Import::Floor => "floor",
            Import::Ceil => "ceil",
            Import::Round => "round",
            Import::Clamp => "clamp",
            Import::Random => "random",
            Import::ToText => "to_text",
            Import::RangeLen => "range_len",
        }
    }

    /// Its parameter and result types.
    pub(crate) fn signature(self) -> (&'static [V], &'static [V]) {
        match self {
            Import::NumAdd | Import::NumSub | Import::NumMul | Import::NumDiv | Import::Random => (&NN, &N),
            Import::NumNeg | Import::Abs | Import::Floor | Import::Ceil | Import::Round => (&N, &N),
            Import::NumCmp => (&NN, &[V::I32]),
            Import::Clamp => (&NNN, &N),
            Import::ToText => (&[V::I64, V::I64, V::I32], &[V::I32]),
            Import::RangeLen => (&N, &[V::I32]),
        }
    }

    /// Its function index.
    pub(crate) fn index(self) -> u32 {
        self as u32
    }
}

/// The items a list may hold, as the `i32` emitted code counts in.
pub(crate) const LIST_ITEMS: u32 = MAX_LIST_SIZE as u32;

/// Frames of the scratch stack: one per nesting depth of collection nodes, and one for the innermost lambda body
/// (R-SBX-19, D-113).
pub const FRAMES: u32 = MAX_COLLECTION_NESTING as u32 + 1;

/// Offset in a frame of the `i32` that is 0, or 1 + the index of the element the collection node at this depth is
/// running its lambda on: the host reads the frames innermost first to name the items of a failure (R-BLT-07, D-118).
pub const FRAME_VISITING: u32 = 0;

/// Offset in a frame of the bytes an import writes into (`to_text`).
pub const FRAME_MARSHAL: u32 = 16;

/// Size of a frame's marshalling bytes: the longest `Number` as text is 31 bytes.
pub const MARSHAL_BYTES: u32 = 48;

/// Offset in a frame of the collection area: what the collection node at this depth holds while it runs.
pub const FRAME_AREA: u32 = FRAME_MARSHAL + MARSHAL_BYTES;

/// Bytes of the collection area one item takes at most: a `map` result before it is stored in its list (a `Number?`
/// is the largest, as its tag and its value; a record is a pointer), or a `sort_by` key of 16 with two `i32` of its
/// order.
pub const AREA_ITEM_BYTES: u32 = 24;

/// Size of a frame's collection area, for a list of `max_list_size` items.
pub const AREA_BYTES: u32 = AREA_ITEM_BYTES * LIST_ITEMS;

/// Size of a frame.
pub const FRAME_BYTES: u32 = FRAME_AREA + AREA_BYTES;

/// S: the size of the whole scratch stack, which starts at address 0 (R-SBX-19, D-113).
pub const SCRATCH_BYTES: u32 = FRAMES * FRAME_BYTES;

/// Where `velme_run` leaves the slot of an output that isn't a record: the collection area of frame 0, free once the
/// body is done. A present `T?` points at its `T` right after the slot, at [`RESULT`] + 8, unless `T` is a record.
pub const RESULT: u32 = FRAME_AREA;

/// Offset in a collection area of the `i32` order `sort_by` sorts, after its keys.
pub(crate) const SORT_ORDER: u32 = 16 * LIST_ITEMS;

/// Offset in a collection area of the second `i32` buffer of the merge sort.
pub(crate) const SORT_SPARE: u32 = SORT_ORDER + 4 * LIST_ITEMS;

/// Bytes before a list's items holding its logical size (`runtime/30` §7.1) as an `i64`, so charging a value that
/// holds the list never walks it. Every list has them, the empty one too, so no list's address is 0 (D-119).
pub const LIST_PREFIX: u32 = 8;

/// Bytes at the start of a record's slot holding its logical size as an `i64`, before its fields: charging a value
/// that holds the record reads it, whatever the record's width (D-119).
pub const RECORD_PREFIX: u32 = 8;

const _: () = assert!(MAX_LIST_SIZE <= u32::MAX as u64 / AREA_ITEM_BYTES as u64);
const _: () = assert!(SORT_SPARE + 4 * LIST_ITEMS <= AREA_BYTES);
const _: () = assert!(FRAME_BYTES.is_multiple_of(8));

/// The address of frame `level`.
pub(crate) fn frame(level: u32) -> u32 {
    level * FRAME_BYTES
}
