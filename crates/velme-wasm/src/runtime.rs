//! The functions every module carries besides its goal's body: the fuel and memory meters, the bump allocator, and
//! the built-ins that walk a `List` or a `Text`, which are emitted code, not host imports (`runtime/31` R-SBX-05,
//! R-SBX-08, D-112). Each mirrors the `velme-builtins` code the interpreter runs and charges the same units in the
//! same order (D-88).

use velme_builtins::TEXT_BLOCK_BYTES;
use velme_builtins::memory::{HEADER_BYTES, NUMBER_BYTES, SLOT_BYTES};
use wasm_encoder::BlockType::Empty;
use wasm_encoder::Instruction as I;

use crate::abi::{Import, LIST_ITEMS, LIST_PREFIX, MARSHAL_BYTES, Reason, SORT_ORDER, SORT_SPARE};
use crate::code::{Assembly, Func, G_FUEL, G_HEAP, G_MEMORY, G_REASON, PAGE_BYTES, V, mem};

/// A runtime function, in function-index order after the imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rt {
    /// `(reason: i32)`: sets `velme_reason` and traps.
    Fail,
    /// `()`: charges one fuel unit.
    Tick,
    /// `(fuel: i64)`: charges fuel.
    ChargeFuel,
    /// `(bytes: i64)`: charges memory.
    ChargeMemory,
    /// `(bytes: i64, fuel: i64)`: charges both, or neither.
    Afford,
    /// `(size: i64) -> i32`: fresh memory, 8-aligned.
    Bump,
    /// `(i64, i64) -> i64`: a saturating sum of sizes.
    SatAdd,
    /// `(len: i64) -> i64`: the logical size of a text.
    TextBytes,
    /// `(len: i64) -> i64`: the fuel blocks of a text.
    Blocks,
    /// `(ptr, len, ptr, len) -> i32`: whether two texts are equal.
    TextEq,
    /// `(ptr, len) -> i32`: the characters of a text.
    TextChars,
    /// `(ptr, len, ptr, len) -> (ptr, len)`: `concat`.
    Concat,
    /// `(lo, hi, marshal: i32) -> (ptr, len)`: `to_text`.
    ToText,
    /// `(lo, hi) -> (ptr, len)`: `range`.
    Range,
    /// `(ptr, len) -> (lo, hi)`: `sum`.
    Sum,
    /// `(ptr, len, want: i32) -> (tag, lo, hi)`: `maximum` for `want` 1, `minimum` for -1.
    Extreme,
    /// `(area: i32, n: i32, descending: i32)`: sorts the order of `sort_by` by the keys in a collection area.
    Sort,
    /// `(size: i32) -> i32`: the export `velme_alloc`.
    Alloc,
}

impl Rt {
    pub(crate) const ALL: [Rt; 18] = [
        Rt::Fail,
        Rt::Tick,
        Rt::ChargeFuel,
        Rt::ChargeMemory,
        Rt::Afford,
        Rt::Bump,
        Rt::SatAdd,
        Rt::TextBytes,
        Rt::Blocks,
        Rt::TextEq,
        Rt::TextChars,
        Rt::Concat,
        Rt::ToText,
        Rt::Range,
        Rt::Sum,
        Rt::Extreme,
        Rt::Sort,
        Rt::Alloc,
    ];

    /// Its function index.
    pub(crate) fn index(self) -> u32 {
        Import::ALL.len() as u32 + self as u32
    }

    /// The instructions of its body ([`Func::instructions`]).
    #[cfg(test)]
    pub(crate) fn instructions(self) -> u64 {
        self.build().instructions()
    }

    fn build(self) -> Func {
        match self {
            Rt::Fail => fail(),
            Rt::Tick => tick(),
            Rt::ChargeFuel => charge(G_FUEL, Reason::OutOfFuel),
            Rt::ChargeMemory => charge(G_MEMORY, Reason::OutOfMemory),
            Rt::Afford => afford(),
            Rt::Bump => bump(),
            Rt::SatAdd => sat_add(),
            Rt::TextBytes => text_bytes(),
            Rt::Blocks => blocks(),
            Rt::TextEq => text_eq(),
            Rt::TextChars => text_chars(),
            Rt::Concat => concat(),
            Rt::ToText => to_text(),
            Rt::Range => range(),
            Rt::Sum => sum(),
            Rt::Extreme => extreme(),
            Rt::Sort => sort(),
            Rt::Alloc => alloc(),
        }
    }
}

/// Defines every runtime function, in index order.
pub(crate) fn define(assembly: &mut Assembly) {
    for rt in Rt::ALL {
        let index = assembly.push(rt.build());
        debug_assert_eq!(index, rt.index());
    }
}

const I32: V = V::I32;
const I64: V = V::I64;

/// An `i64` constant of a size or count.
fn size(n: u64) -> I<'static> {
    I::I64Const(n.cast_signed())
}

/// Traps with `reason`.
fn trap(f: &mut Func, reason: Reason) {
    f.ops([I::I32Const(reason.code()), I::Call(Rt::Fail.index())]);
}

fn fail() -> Func {
    let mut f = Func::new(&[I32], &[]);
    f.ops([I::LocalGet(0), I::GlobalSet(G_REASON), I::Unreachable]);
    f
}

/// Leaves the meter `global` at 0 and traps with `reason`: what was spent is then the limit itself, as the
/// interpreter's `Evaluator::spent` clamps it.
fn exhausted(f: &mut Func, global: u32, reason: Reason) {
    f.ops([I::I64Const(0), I::GlobalSet(global)]);
    trap(f, reason);
}

/// `Evaluator::charge(1)`.
fn tick() -> Func {
    let mut f = Func::new(&[], &[]);
    f.ops([I::GlobalGet(G_FUEL), I::I64Eqz, I::If(Empty)]);
    exhausted(&mut f, G_FUEL, Reason::OutOfFuel);
    f.ops([
        I::End,
        I::GlobalGet(G_FUEL),
        I::I64Const(1),
        I::I64Sub,
        I::GlobalSet(G_FUEL),
    ]);
    f
}

/// `Evaluator::charge` and `Evaluator::allocate`: spending more than is left is the failure, and the meter counts
/// down, so nothing saturates or wraps: a total that would pass `u64::MAX` fails, as the interpreter's does.
fn charge(global: u32, reason: Reason) -> Func {
    let mut f = Func::new(&[I64], &[]);
    f.ops([I::LocalGet(0), I::GlobalGet(global), I::I64GtU, I::If(Empty)]);
    exhausted(&mut f, global, reason);
    f.ops([
        I::End,
        I::GlobalGet(global),
        I::LocalGet(0),
        I::I64Sub,
        I::GlobalSet(global),
    ]);
    f
}

/// The `afford` of `Function::call`: the result's bytes are checked first, then the fuel, and only a call that can
/// pay for both is charged either (D-88).
fn afford() -> Func {
    let mut f = Func::new(&[I64, I64], &[]);
    f.ops([I::LocalGet(0), I::GlobalGet(G_MEMORY), I::I64GtU, I::If(Empty)]);
    exhausted(&mut f, G_MEMORY, Reason::OutOfMemory);
    f.ops([I::End, I::LocalGet(1), I::GlobalGet(G_FUEL), I::I64GtU, I::If(Empty)]);
    exhausted(&mut f, G_FUEL, Reason::OutOfFuel);
    f.ops([
        I::End,
        I::GlobalGet(G_MEMORY),
        I::LocalGet(0),
        I::I64Sub,
        I::GlobalSet(G_MEMORY),
    ]);
    f.ops([I::GlobalGet(G_FUEL), I::LocalGet(1), I::I64Sub, I::GlobalSet(G_FUEL)]);
    f
}

/// The bump allocator (R-SBX-03): nothing is freed, and fresh memory is zero. It grows the memory as it goes, and a
/// refusal is the host's backstop (R-SBX-12).
fn bump() -> Func {
    let mut f = Func::new(&[I64], &[I32]);
    let end = f.local(I64);
    // Nothing a run may allocate is near 4 GiB; refusing here keeps the sums below from wrapping.
    f.ops([
        I::LocalGet(0),
        I::I64Const(i64::from(u32::MAX)),
        I::I64GtU,
        I::If(Empty),
    ]);
    trap(&mut f, Reason::GrowRefused);
    f.op(I::End);
    f.ops([I::GlobalGet(G_HEAP), I::I64ExtendI32U]);
    f.ops([I::LocalGet(0), I::I64Const(7), I::I64Add, I::I64Const(-8), I::I64And]);
    f.ops([I::I64Add, I::LocalSet(end)]);
    let pages = PAGE_BYTES.cast_signed();
    f.ops([
        I::LocalGet(end),
        I::MemorySize(0),
        I::I64ExtendI32U,
        I::I64Const(pages),
        I::I64Mul,
    ]);
    f.ops([I::I64GtU, I::If(Empty)]);
    f.ops([
        I::LocalGet(end),
        I::MemorySize(0),
        I::I64ExtendI32U,
        I::I64Const(pages),
        I::I64Mul,
    ]);
    f.ops([
        I::I64Sub,
        I::I64Const(pages - 1),
        I::I64Add,
        I::I64Const(pages),
        I::I64DivU,
    ]);
    f.ops([I::I32WrapI64, I::MemoryGrow(0), I::I32Const(-1), I::I32Eq, I::If(Empty)]);
    trap(&mut f, Reason::GrowRefused);
    f.ops([I::End, I::End]);
    f.ops([
        I::GlobalGet(G_HEAP),
        I::LocalGet(end),
        I::I32WrapI64,
        I::GlobalSet(G_HEAP),
    ]);
    f
}

/// `u64::saturating_add`, as `velme_builtins::memory` sums sizes.
fn sat_add() -> Func {
    let mut f = Func::new(&[I64, I64], &[I64]);
    let total = f.local(I64);
    f.ops([I::LocalGet(0), I::LocalGet(1), I::I64Add, I::LocalSet(total)]);
    f.ops([I::I64Const(-1), I::LocalGet(total)]);
    f.ops([I::LocalGet(total), I::LocalGet(0), I::I64LtU, I::Select]);
    f
}

/// `velme_builtins::memory::text_bytes`.
fn text_bytes() -> Func {
    let mut f = Func::new(&[I64], &[I64]);
    f.ops([
        I::LocalGet(0),
        size(SLOT_BYTES - 1),
        I::I64Add,
        size(SLOT_BYTES),
        I::I64DivU,
    ]);
    f.ops([size(SLOT_BYTES), I::I64Mul, size(HEADER_BYTES), I::I64Add]);
    f
}

/// The `blocks` of `velme_builtins`: ⌈bytes/64⌉ (D-52).
fn blocks() -> Func {
    let mut f = Func::new(&[I64], &[I64]);
    f.ops([
        I::LocalGet(0),
        size(TEXT_BLOCK_BYTES - 1),
        I::I64Add,
        size(TEXT_BLOCK_BYTES),
        I::I64DivU,
    ]);
    f
}

/// Opens `block { loop {` and leaves when `i >= n`: inside, `br 0` is the next turn and `br 1` the way out.
fn open_loop(f: &mut Func, i: u32, n: u32) {
    f.ops([I::Block(Empty), I::Loop(Empty)]);
    f.ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
}

/// Adds 1 to `i` and closes what [`open_loop`] opened.
fn close_loop(f: &mut Func, i: u32) {
    f.ops([I::LocalGet(i), I::I32Const(1), I::I32Add, I::LocalSet(i)]);
    f.ops([I::Br(0), I::End, I::End]);
}

/// The text arm of `equal` in `velme_builtins`: texts of different lengths differ before any byte is compared, and
/// equal lengths cost their blocks before the bytes are (D-83).
fn text_eq() -> Func {
    let mut f = Func::new(&[I32, I32, I32, I32], &[I32]);
    let i = f.local(I32);
    f.ops([
        I::LocalGet(1),
        I::LocalGet(3),
        I::I32Ne,
        I::If(Empty),
        I::I32Const(0),
        I::Return,
        I::End,
    ]);
    f.ops([I::LocalGet(1), I::I64ExtendI32U, I::Call(Rt::Blocks.index())]);
    f.ops([I::Call(Rt::ChargeFuel.index()), I::I32Const(0), I::LocalSet(i)]);
    open_loop(&mut f, i, 1);
    f.ops([I::LocalGet(0), I::LocalGet(i), I::I32Add, I::I32Load8U(mem(0, 0))]);
    f.ops([I::LocalGet(2), I::LocalGet(i), I::I32Add, I::I32Load8U(mem(0, 0))]);
    f.ops([I::I32Ne, I::If(Empty), I::I32Const(0), I::Return, I::End]);
    close_loop(&mut f, i);
    f.op(I::I32Const(1));
    f
}

/// `str::chars().count()`: every byte of valid UTF-8 that isn't a continuation byte starts a character.
fn text_chars() -> Func {
    let mut f = Func::new(&[I32, I32], &[I32]);
    let (i, count) = (f.local(I32), f.local(I32));
    f.ops([I::I32Const(0), I::LocalSet(i), I::I32Const(0), I::LocalSet(count)]);
    open_loop(&mut f, i, 1);
    f.ops([I::LocalGet(0), I::LocalGet(i), I::I32Add, I::I32Load8U(mem(0, 0))]);
    f.ops([I::I32Const(0xC0), I::I32And, I::I32Const(0x80), I::I32Ne]);
    f.ops([I::LocalGet(count), I::I32Add, I::LocalSet(count)]);
    close_loop(&mut f, i);
    f.op(I::LocalGet(count));
    f
}

/// `Function::Concat`.
fn concat() -> Func {
    let mut f = Func::new(&[I32, I32, I32, I32], &[I32, I32]);
    let (total, at) = (f.local(I64), f.local(I32));
    f.ops([
        I::LocalGet(1),
        I::I64ExtendI32U,
        I::LocalGet(3),
        I::I64ExtendI32U,
        I::I64Add,
    ]);
    f.op(I::LocalSet(total));
    f.ops([I::LocalGet(total), I::Call(Rt::TextBytes.index())]);
    f.ops([
        I::LocalGet(total),
        I::Call(Rt::Blocks.index()),
        I::Call(Rt::Afford.index()),
    ]);
    f.ops([I::LocalGet(total), I::Call(Rt::Bump.index()), I::LocalSet(at)]);
    f.ops([I::LocalGet(at), I::LocalGet(0), I::LocalGet(1), COPY]);
    f.ops([
        I::LocalGet(at),
        I::LocalGet(1),
        I::I32Add,
        I::LocalGet(2),
        I::LocalGet(3),
        COPY,
    ]);
    f.ops([I::LocalGet(at), I::LocalGet(total), I::I32WrapI64]);
    f
}

/// `memory.copy` within the one memory (bulk memory, R-SBX-07).
pub(crate) const COPY: I<'static> = I::MemoryCopy { src_mem: 0, dst_mem: 0 };

/// `Function::ToText`: the host writes the text; its memory is charged, then its fuel, as `Evaluator::settle` does.
fn to_text() -> Func {
    let mut f = Func::new(&[I64, I64, I32], &[I32, I32]);
    let (len, at) = (f.local(I32), f.local(I32));
    f.ops([
        I::LocalGet(0),
        I::LocalGet(1),
        I::LocalGet(2),
        I::Call(Import::ToText.index()),
    ]);
    f.ops([
        I::LocalTee(len),
        I::I32Const(MARSHAL_BYTES.cast_signed()),
        I::I32GtU,
        I::If(Empty),
    ]);
    trap(&mut f, Reason::Internal);
    f.op(I::End);
    f.ops([I::LocalGet(len), I::I64ExtendI32U, I::Call(Rt::TextBytes.index())]);
    f.op(I::Call(Rt::ChargeMemory.index()));
    f.ops([I::LocalGet(len), I::I64ExtendI32U, I::Call(Rt::Blocks.index())]);
    f.op(I::Call(Rt::ChargeFuel.index()));
    f.ops([
        I::LocalGet(len),
        I::I64ExtendI32U,
        I::Call(Rt::Bump.index()),
        I::LocalSet(at),
    ]);
    f.ops([I::LocalGet(at), I::LocalGet(2), I::LocalGet(len), COPY]);
    f.ops([I::LocalGet(at), I::LocalGet(len)]);
    f
}

/// `Function::Range`: the host checks the count, which may fail (R-RUN-04: a size error wins), and the list is
/// filled here.
fn range() -> Func {
    let mut f = Func::new(&[I64, I64], &[I32, I32]);
    let (len, at, i, bytes) = (f.local(I32), f.local(I32), f.local(I32), f.local(I64));
    f.ops([I::LocalGet(0), I::LocalGet(1), I::Call(Import::RangeLen.index())]);
    f.ops([
        I::LocalTee(len),
        I::I32Const(LIST_ITEMS.cast_signed()),
        I::I32GtU,
        I::If(Empty),
    ]);
    trap(&mut f, Reason::Internal);
    f.op(I::End);
    // `number_list_bytes`.
    f.ops([I::LocalGet(len), I::I64ExtendI32U, size(NUMBER_BYTES), I::I64Mul]);
    f.ops([size(HEADER_BYTES), I::I64Add, I::LocalSet(bytes)]);
    f.ops([
        I::LocalGet(bytes),
        I::LocalGet(len),
        I::I64ExtendI32U,
        I::Call(Rt::Afford.index()),
    ]);
    f.ops([I::LocalGet(len), I::I64ExtendI32U, I::I64Const(16), I::I64Mul]);
    f.ops([
        size(u64::from(LIST_PREFIX)),
        I::I64Add,
        I::Call(Rt::Bump.index()),
        I::LocalSet(at),
    ]);
    f.ops([I::LocalGet(at), I::LocalGet(bytes), I::I64Store(mem(0, 3))]);
    f.ops([
        I::LocalGet(at),
        I::I32Const(LIST_PREFIX.cast_signed()),
        I::I32Add,
        I::LocalSet(at),
    ]);
    f.ops([I::I32Const(0), I::LocalSet(i)]);
    open_loop(&mut f, i, len);
    // An integer below 2^64 is its own `lo`, with `hi` 0 (D-113).
    f.ops([I::LocalGet(at), I::LocalGet(i), I::I32Const(16), I::I32Mul, I::I32Add]);
    f.ops([I::LocalGet(i), I::I64ExtendI32U, I::I64Store(mem(0, 3))]);
    f.ops([I::LocalGet(at), I::LocalGet(i), I::I32Const(16), I::I32Mul, I::I32Add]);
    f.ops([I::I64Const(0), I::I64Store(mem(8, 3))]);
    close_loop(&mut f, i);
    f.ops([I::LocalGet(at), I::LocalGet(len)]);
    f
}

/// Pushes the `Number` in the slot at `at + 16 * i`.
fn number_at(f: &mut Func, at: u32, i: u32) {
    for offset in [0, 8] {
        f.ops([I::LocalGet(at), I::LocalGet(i), I::I32Const(16), I::I32Mul, I::I32Add]);
        f.op(I::I64Load(mem(offset, 3)));
    }
}

/// `Function::Sum`: left to right from 0, and an overflow wins over the fuel, which is charged after.
fn sum() -> Func {
    let mut f = Func::new(&[I32, I32], &[I64, I64]);
    let (i, lo, hi) = (f.local(I32), f.local(I64), f.local(I64));
    f.ops([
        I::I32Const(0),
        I::LocalSet(i),
        I::I64Const(0),
        I::LocalSet(lo),
        I::I64Const(0),
        I::LocalSet(hi),
    ]);
    open_loop(&mut f, i, 1);
    f.ops([I::LocalGet(lo), I::LocalGet(hi)]);
    number_at(&mut f, 0, i);
    f.ops([I::Call(Import::NumAdd.index()), I::LocalSet(hi), I::LocalSet(lo)]);
    close_loop(&mut f, i);
    f.ops([I::LocalGet(1), I::I64ExtendI32U, I::Call(Rt::ChargeFuel.index())]);
    f.ops([I::LocalGet(lo), I::LocalGet(hi)]);
    f
}

/// `Function::Maximum` and `Function::Minimum`: nothing can fail, so the fuel is charged before the work it pays for.
/// `want` is what `num_cmp` gives for an item that replaces the best so far: exactly 1 or -1 (D-119).
fn extreme() -> Func {
    let mut f = Func::new(&[I32, I32, I32], &[I32, I64, I64]);
    let (i, lo, hi) = (f.local(I32), f.local(I64), f.local(I64));
    f.ops([I::LocalGet(1), I::I64ExtendI32U, I::Call(Rt::ChargeFuel.index())]);
    f.ops([I::LocalGet(1), I::I32Eqz, I::If(Empty)]);
    f.ops([I::I32Const(0), I::I64Const(0), I::I64Const(0), I::Return, I::End]);
    f.ops([I::I32Const(0), I::LocalSet(i)]);
    number_at(&mut f, 0, i);
    f.ops([I::LocalSet(hi), I::LocalSet(lo), I::I32Const(1), I::LocalSet(i)]);
    open_loop(&mut f, i, 1);
    number_at(&mut f, 0, i);
    f.ops([I::LocalGet(lo), I::LocalGet(hi), I::Call(Import::NumCmp.index())]);
    f.ops([I::LocalGet(2), I::I32Eq, I::If(Empty)]);
    number_at(&mut f, 0, i);
    f.ops([I::LocalSet(hi), I::LocalSet(lo), I::End]);
    close_loop(&mut f, i);
    f.ops([I::I32Const(1), I::LocalGet(lo), I::LocalGet(hi)]);
    f
}

/// The order of `velme_builtins::sort_order`: a stable bottom-up merge sort of the indexes `0..n` by their keys,
/// which are at the start of the collection area. It compares at most `n·⌈log2 n⌉` times, within what `sort_by`
/// has paid (D-52). Equal keys keep their input order, `descending` too (R-BLT-11, D-59).
fn sort() -> Func {
    let (area, n, descending) = (0, 1, 2);
    let mut f = Func::new(&[I32, I32, I32], &[]);
    let [from, to, width, low, mid, high, a, b, k, c] = [(); 10].map(|()| f.local(I32));
    let order = SORT_ORDER.cast_signed();
    f.ops([I::LocalGet(area), I::I32Const(order), I::I32Add, I::LocalSet(from)]);
    f.ops([
        I::LocalGet(area),
        I::I32Const(SORT_SPARE.cast_signed()),
        I::I32Add,
        I::LocalSet(to),
    ]);
    f.ops([I::I32Const(0), I::LocalSet(k)]);
    open_loop(&mut f, k, n);
    f.ops([I::LocalGet(from), I::LocalGet(k), I::I32Const(4), I::I32Mul, I::I32Add]);
    f.ops([I::LocalGet(k), I::I32Store(mem(0, 2))]);
    close_loop(&mut f, k);
    // The `i32` at index `at` of the buffer `buffer`.
    let index = |f: &mut Func, buffer: u32, at: u32| {
        f.ops([
            I::LocalGet(buffer),
            I::LocalGet(at),
            I::I32Const(4),
            I::I32Mul,
            I::I32Add,
        ]);
    };
    // `min(x + width, n)`, into `into`.
    let step = |f: &mut Func, x: u32, into: u32| {
        f.ops([I::LocalGet(x), I::LocalGet(width), I::I32Add, I::LocalTee(into)]);
        f.ops([
            I::LocalGet(n),
            I::LocalGet(into),
            I::LocalGet(n),
            I::I32LtU,
            I::Select,
            I::LocalSet(into),
        ]);
    };
    f.ops([I::I32Const(1), I::LocalSet(width)]);
    f.ops([I::Block(Empty), I::Loop(Empty)]);
    f.ops([I::LocalGet(width), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
    f.ops([I::I32Const(0), I::LocalSet(low)]);
    f.ops([I::Block(Empty), I::Loop(Empty)]);
    f.ops([I::LocalGet(low), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
    step(&mut f, low, mid);
    step(&mut f, mid, high);
    f.ops([I::LocalGet(low), I::LocalSet(a), I::LocalGet(mid), I::LocalSet(b)]);
    f.ops([I::LocalGet(low), I::LocalSet(k)]);
    open_loop(&mut f, k, high);
    // Take from the right run only when the left is used up, or the right key sorts strictly first.
    f.ops([
        I::LocalGet(a),
        I::LocalGet(mid),
        I::I32GeU,
        I::If(wasm_encoder::BlockType::Result(wasm_encoder::ValType::I32)),
    ]);
    f.op(I::I32Const(1));
    f.op(I::Else);
    f.ops([
        I::LocalGet(b),
        I::LocalGet(high),
        I::I32GeU,
        I::If(wasm_encoder::BlockType::Result(wasm_encoder::ValType::I32)),
    ]);
    f.op(I::I32Const(0));
    f.op(I::Else);
    for run in [b, a] {
        index(&mut f, from, run);
        f.ops([I::I32Load(mem(0, 2)), I::LocalSet(c)]);
        number_at(&mut f, area, c);
    }
    f.ops([I::Call(Import::NumCmp.index()), I::LocalSet(c)]);
    f.ops([I::LocalGet(c), I::I32Const(0), I::I32GtS]);
    f.ops([I::LocalGet(c), I::I32Const(0), I::I32LtS]);
    f.ops([I::LocalGet(descending), I::Select]);
    f.ops([I::End, I::End]);
    f.op(I::If(Empty));
    index(&mut f, to, k);
    index(&mut f, from, b);
    f.ops([I::I32Load(mem(0, 2)), I::I32Store(mem(0, 2))]);
    f.ops([I::LocalGet(b), I::I32Const(1), I::I32Add, I::LocalSet(b)]);
    f.op(I::Else);
    index(&mut f, to, k);
    index(&mut f, from, a);
    f.ops([I::I32Load(mem(0, 2)), I::I32Store(mem(0, 2))]);
    f.ops([I::LocalGet(a), I::I32Const(1), I::I32Add, I::LocalSet(a)]);
    f.op(I::End);
    close_loop(&mut f, k);
    f.ops([
        I::LocalGet(low),
        I::LocalGet(width),
        I::I32Const(2),
        I::I32Mul,
        I::I32Add,
        I::LocalSet(low),
    ]);
    f.ops([I::Br(0), I::End, I::End]);
    // The merged runs are the next pass's input.
    f.ops([I::LocalGet(from), I::LocalGet(to), I::LocalSet(from), I::LocalSet(to)]);
    f.ops([I::LocalGet(width), I::I32Const(2), I::I32Mul, I::LocalSet(width)]);
    f.ops([I::Br(0), I::End, I::End]);
    // The caller reads the order where it started.
    f.ops([I::LocalGet(area), I::I32Const(order), I::I32Add, I::LocalTee(to)]);
    f.ops([I::LocalGet(from), I::I32Ne, I::If(Empty)]);
    f.ops([
        I::LocalGet(to),
        I::LocalGet(from),
        I::LocalGet(n),
        I::I32Const(4),
        I::I32Mul,
        COPY,
    ]);
    f.op(I::End);
    f
}

/// `velme_alloc` (R-SBX-03): uncharged, since the inputs are the caller's (D-53).
fn alloc() -> Func {
    let mut f = Func::new(&[I32], &[I32]);
    f.ops([I::LocalGet(0), I::I64ExtendI32U, I::Call(Rt::Bump.index())]);
    f
}
