//! A leaf goal's body as the function `velme_run` (`runtime/31` §4). Each node is emitted by the method named after
//! the interpreter code it mirrors (`velme-interp`, `Evaluator::node`), and charges the same fuel and memory in the
//! same order, so both backends stop at the same limit with the same figures (R-SBX-05, D-88, INV-3).

use std::collections::BTreeMap;

use velme_builtins::memory::{HEADER_BYTES, OPTIONAL_BYTES};
use velme_builtins::{Builtin, Function};
use velme_ir::{BinaryOperator, Goal, Lambda, Node, ReduceLambda, UnaryOperator};
use wasm_encoder::BlockType::{self, Empty};
use wasm_encoder::{Instruction as I, ValType};

use crate::EmitError;
use crate::abi::{
    AREA_ITEM_BYTES, FRAME_AREA, FRAME_MARSHAL, FRAME_VISITING, FRAMES, Import, LIST_ITEMS, LIST_PREFIX, RESULT,
    Reason, SORT_ORDER, frame,
};
use crate::code::{Assembly, Func, V, mem};
use crate::data::{Const, Data};
use crate::runtime::{self, COPY, Rt};
use crate::ty::{Ty, Types, Typing, assignable, join};

/// The parameter of `velme_run`: the address of the inputs, each in its slot, in declared order.
const INPUTS: u32 = 0;

/// A block leaving one `i32`.
const ONE_I32: BlockType = BlockType::Result(ValType::I32);

fn internal() -> EmitError {
    EmitError::Internal("the IR has a node the validator should have rejected".to_owned())
}

/// An address or a count below 2^31 as an `i32` constant.
fn int(n: u32) -> I<'static> {
    I::I32Const(n.cast_signed())
}

/// A size as an `i64` constant.
fn size(n: u64) -> I<'static> {
    I::I64Const(n.cast_signed())
}

/// The module for the leaf goal `goal`, whose body is `body`, and the bytes its data segment takes.
pub(crate) fn module(goal: &Goal, body: &Node) -> Result<(Vec<u8>, u32), EmitError> {
    if !goal.calls.is_empty() {
        return Err(EmitError::NotLeaf);
    }
    let types = Types::new(goal)?;
    let mut inputs = Vec::with_capacity(goal.inputs.len());
    let mut offset = 0u32;
    for (name, ty) in &goal.inputs {
        let ty = types.resolve(ty)?;
        let slot = types.slot(&ty)?;
        inputs.push((name.as_str(), ty, offset));
        offset = offset
            .checked_add(slot)
            .filter(|end| i32::try_from(*end).is_ok())
            .ok_or(EmitError::Declined("inputs too wide for one memory"))?;
    }
    let output = types.resolve(&goal.output)?;
    let scope: Vec<(&str, Ty)> = inputs.iter().map(|(name, ty, _)| (*name, ty.clone())).collect();
    let typing = Typing::new(&types, &scope, body)?;
    let mut assembly = Assembly::default();
    runtime::define(&mut assembly);
    let run = assembly.reserve();
    let mut emitter = Emitter {
        types,
        typing,
        assembly,
        data: Data::default(),
        f: Func::new(&[V::I32], &[V::I32]),
        inputs,
        scope: Vec::new(),
        level: 0,
        list_equals: BTreeMap::new(),
        record_equals: BTreeMap::new(),
    };
    emitter.expr(body)?;
    let found = emitter.ty(body)?;
    emitter.coerce(&found, &output)?;
    let vals = emitter.pop(&output);
    emitter.result(&output, &vals)?;
    let Emitter {
        mut assembly, data, f, ..
    } = emitter;
    assembly.define(run, f);
    let segment = u32::try_from(data.bytes.len().next_multiple_of(8)).map_err(|_| internal())?;
    let bytes = assembly
        .finish(&data.bytes, Rt::Alloc.index(), run)
        .ok_or_else(|| EmitError::Internal("a function was called and never emitted".to_owned()))?;
    Ok((bytes, segment))
}

struct Emitter<'ir> {
    types: Types,
    typing: Typing,
    assembly: Assembly,
    data: Data,
    /// The function being emitted: `velme_run`, or a helper it needs.
    f: Func,
    /// The goal's inputs, each with its type and where its slot starts from [`INPUTS`].
    inputs: Vec<(&'ir str, Ty, u32)>,
    /// `let` names and lambda parameters in scope, each with the locals holding its flat value; names never repeat
    /// (R-IR-12).
    scope: Vec<(&'ir str, Vec<u32>)>,
    /// How many lambdas the code being emitted is inside: the scratch frame it may use (R-SBX-19).
    level: u32,
    /// The helper comparing two lists, by item type.
    list_equals: BTreeMap<Ty, u32>,
    /// The helper comparing two records, by record.
    record_equals: BTreeMap<usize, u32>,
}

/// A collection node's loop over its list, open at the point its lambda runs.
struct Loop {
    /// The index of the element.
    i: u32,
    /// The element's flat value: the lambda's parameter.
    param: Vec<u32>,
    /// The scope before the parameter.
    mark: usize,
}

impl<'ir> Emitter<'ir> {
    fn ty(&self, node: &Node) -> Result<Ty, EmitError> {
        self.typing.of(node).cloned()
    }

    fn call(&mut self, rt: Rt) {
        self.f.call(rt.index());
    }

    fn import(&mut self, import: Import) {
        self.f.call(import.index());
    }

    /// Traps with `reason`.
    fn trap(&mut self, reason: Reason) {
        self.f.op(I::I32Const(reason.code()));
        self.call(Rt::Fail);
    }

    /// Moves the flat value of type `ty` on the stack into fresh locals.
    fn pop(&mut self, ty: &Ty) -> Vec<u32> {
        let flat = self.types.flat(ty);
        let locals: Vec<u32> = flat.into_iter().map(|v| self.f.local(v)).collect();
        self.f.set(&locals);
        locals
    }

    /// Pushes a zero for each part of the flat form of `ty`.
    fn zeros(&mut self, ty: &Ty) {
        for v in self.types.flat(ty) {
            self.f.op(match v {
                V::I32 => I::I32Const(0),
                V::I64 => I::I64Const(0),
            });
        }
    }

    /// Turns the value of type `from` on the stack into one of `to`, which it is assignable to (R-TYP-20): the
    /// interpreter's values need no such step, since a present `T?` is the `T` itself.
    fn coerce(&mut self, from: &Ty, to: &Ty) -> Result<(), EmitError> {
        if from == to {
            return Ok(());
        }
        match to {
            Ty::Optional(of) if *from == Ty::Nothing => {
                self.f.op(I::I32Const(0));
                self.zeros(of);
            }
            Ty::Optional(of) if from == &**of => {
                let vals = self.pop(of);
                self.f.op(I::I32Const(1));
                self.f.get(&vals);
                self.f.release(&vals);
            }
            _ => return Err(internal()),
        }
        Ok(())
    }

    /// Pushes the flat value of type `ty` stored at `base + offset`. `inline` is a slot of §3: a record is its
    /// fields, and its flat value is that address; an optional is a pointer to its `T`, or 0. Otherwise it is a
    /// scratch cell, which holds the flat value as it is: a record's pointer, an optional's tag and then its `T`.
    fn load(&mut self, ty: &Ty, base: u32, offset: u32, inline: bool) -> Result<(), EmitError> {
        match ty {
            Ty::Number => {
                self.f.ops([I::LocalGet(base), I::I64Load(mem(offset, 3))]);
                self.f.ops([I::LocalGet(base), I::I64Load(mem(offset + 8, 3))]);
            }
            Ty::Nothing => {}
            Ty::Text | Ty::List(_) => {
                self.f.ops([I::LocalGet(base), I::I32Load(mem(offset, 2))]);
                self.f.ops([I::LocalGet(base), I::I32Load(mem(offset + 4, 2))]);
            }
            Ty::Optional(of) if inline => {
                let at = self.f.local(V::I32);
                self.f
                    .ops([I::LocalGet(base), I::I32Load(mem(offset, 2)), I::LocalSet(at)]);
                if matches!(**of, Ty::Record(_)) {
                    // It points at the record, whose flat value is that address.
                    self.f.ops([I::LocalGet(at), I::I32Const(0), I::I32Ne, I::LocalGet(at)]);
                } else {
                    let block = self.assembly.block(&self.types.flat(ty));
                    self.f.ops([I::LocalGet(at), I::If(block), I::I32Const(1)]);
                    self.load(of, at, 0, true)?;
                    self.f.ops([I::Else, I::I32Const(0)]);
                    self.zeros(of);
                    self.f.op(I::End);
                }
                self.f.release(&[at]);
            }
            Ty::Optional(of) => {
                self.f.ops([I::LocalGet(base), I::I32Load(mem(offset, 2))]);
                self.load(of, base, offset + 8, false)?;
            }
            Ty::Record(_) if inline => self.f.ops([I::LocalGet(base), int(offset), I::I32Add]),
            Ty::Boolean | Ty::Record(_) => self.f.ops([I::LocalGet(base), I::I32Load(mem(offset, 2))]),
        }
        Ok(())
    }

    /// Stores the flat value in `vals`, of type `ty`, at `base + offset`; `inline` as in [`Emitter::load`]. The `T`
    /// of a present optional stored inline takes fresh memory, within what the value holding it was charged
    /// (`runtime/30` §7.1, R-SBX-19); a record is pointed at where it is.
    fn store(&mut self, ty: &Ty, base: u32, offset: u32, vals: &[u32], inline: bool) -> Result<(), EmitError> {
        match (ty, vals) {
            (Ty::Number, &[lo, hi]) => {
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(lo), I::I64Store(mem(offset, 3))]);
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(hi), I::I64Store(mem(offset + 8, 3))]);
            }
            (Ty::Nothing, []) => {}
            (Ty::Text | Ty::List(_), &[ptr, len]) => {
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(ptr), I::I32Store(mem(offset, 2))]);
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(len), I::I32Store(mem(offset + 4, 2))]);
            }
            (Ty::Optional(of), &[tag, ptr]) if inline && matches!(**of, Ty::Record(_)) => {
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(ptr), I::I32Const(0), I::LocalGet(tag)]);
                self.f.ops([I::Select, I::I32Store(mem(offset, 2))]);
            }
            (Ty::Optional(of), [tag, rest @ ..]) if inline => {
                let slot = self.types.slot(of)?;
                let boxed = self.f.local(V::I32);
                self.f.ops([I::LocalGet(*tag), I::If(Empty), size(u64::from(slot))]);
                self.call(Rt::Bump);
                self.f.op(I::LocalSet(boxed));
                self.store(of, boxed, 0, rest, true)?;
                self.f.ops([I::Else, I::I32Const(0), I::LocalSet(boxed), I::End]);
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(boxed), I::I32Store(mem(offset, 2))]);
                self.f.release(&[boxed]);
            }
            (Ty::Optional(of), [tag, rest @ ..]) => {
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(*tag), I::I32Store(mem(offset, 2))]);
                self.store(of, base, offset + 8, rest, false)?;
            }
            (Ty::Record(index), &[ptr]) if inline => {
                let slot = self.types.record(*index)?.slot;
                self.f.ops([I::LocalGet(base), int(offset), I::I32Add]);
                self.f.ops([I::LocalGet(ptr), int(slot), COPY]);
            }
            (Ty::Boolean | Ty::Record(_), &[value]) => {
                self.f
                    .ops([I::LocalGet(base), I::LocalGet(value), I::I32Store(mem(offset, 2))]);
            }
            _ => return Err(internal()),
        }
        Ok(())
    }

    /// Pushes what `velme_run` returns for the output in `vals`, of type `ty`: a record's own address, and
    /// [`RESULT`] for any other type, its slot stored there (R-SBX-03). A result that is a scalar on its own is
    /// charged nothing (D-89), so the `T` of a present `T?` goes in the frame too, right after the slot.
    fn result(&mut self, ty: &Ty, vals: &[u32]) -> Result<(), EmitError> {
        if let (Ty::Record(_), &[ptr]) = (ty, vals) {
            self.f.op(I::LocalGet(ptr));
            return Ok(());
        }
        let at = self.f.local(V::I32);
        match (ty, vals) {
            (Ty::Optional(of), [tag, rest @ ..]) if !matches!(**of, Ty::Record(_)) => {
                self.f
                    .ops([int(RESULT + 8), I::LocalSet(at), I::LocalGet(*tag), I::If(Empty)]);
                self.store(of, at, 0, rest, true)?;
                self.f.op(I::End);
                self.f
                    .ops([int(RESULT), I::LocalGet(at), I::I32Const(0), I::LocalGet(*tag)]);
                self.f.ops([I::Select, I::I32Store(mem(0, 2))]);
            }
            _ => {
                self.f.ops([int(RESULT), I::LocalSet(at)]);
                self.store(ty, at, 0, vals, true)?;
            }
        }
        self.f.op(int(RESULT));
        Ok(())
    }

    /// Pushes the logical size of the value in `vals`, of type `ty`: `value_bytes` of `velme_builtins::memory`
    /// (`runtime/30` §7.1). A list and a record know their sizes, so nothing is walked.
    fn bytes(&mut self, ty: &Ty, vals: &[u32]) -> Result<(), EmitError> {
        if let Some(fixed) = self.types.fixed_bytes(ty) {
            self.f.op(size(fixed));
            return Ok(());
        }
        match (ty, vals) {
            (Ty::Text, &[_, len]) => {
                self.f.ops([I::LocalGet(len), I::I64ExtendI32U]);
                self.call(Rt::TextBytes);
            }
            (Ty::List(_), &[ptr, _]) => {
                self.f
                    .ops([I::LocalGet(ptr), int(LIST_PREFIX), I::I32Sub, I::I64Load(mem(0, 3))]);
            }
            (Ty::Record(_), &[ptr]) => self.f.ops([I::LocalGet(ptr), I::I64Load(mem(0, 3))]),
            (Ty::Optional(of), [tag, rest @ ..]) => {
                self.f.ops([I::LocalGet(*tag), I::If(BlockType::Result(ValType::I64))]);
                self.bytes(of, rest)?;
                self.f.ops([I::Else, I::I64Const(0), I::End]);
            }
            _ => return Err(internal()),
        }
        Ok(())
    }

    /// Adds to the `i64` local `total` what the value in `vals` takes as a list item or record field: `slot_bytes`
    /// of `velme_builtins::memory`, its size and 8 more if its type is optional.
    fn add_bytes(&mut self, total: u32, ty: &Ty, vals: &[u32]) -> Result<(), EmitError> {
        self.f.op(I::LocalGet(total));
        self.bytes(ty, vals)?;
        self.call(Rt::SatAdd);
        if matches!(ty, Ty::Optional(_)) {
            self.f.op(size(OPTIONAL_BYTES));
            self.call(Rt::SatAdd);
        }
        self.f.op(I::LocalSet(total));
        Ok(())
    }

    /// Emits a helper function with `build`, which may call the helper itself: a type can hold itself through a list.
    fn helper(
        &mut self,
        index: u32,
        params: &[V],
        results: &[V],
        build: impl FnOnce(&mut Self) -> Result<(), EmitError>,
    ) -> Result<(), EmitError> {
        let outer = std::mem::replace(&mut self.f, Func::new(params, results));
        let built = build(self);
        let helper = std::mem::replace(&mut self.f, outer);
        self.assembly.define(index, helper);
        built
    }

    /// Charges the pair of `a` and `b`, if either is a list or a record: the `pairs` of `equal` in `velme_builtins`,
    /// as `equals` and `contains` first work it out (D-83).
    fn charge_pair(&mut self, ty: &Ty, a: &[u32], b: &[u32]) {
        match (ty, a, b) {
            (Ty::List(_) | Ty::Record(_), ..) => self.call(Rt::Tick),
            (Ty::Optional(of), [a, ..], [b, ..]) if matches!(**of, Ty::List(_) | Ty::Record(_)) => {
                self.f.ops([I::LocalGet(*a), I::LocalGet(*b), I::I32Or, I::If(Empty)]);
                self.call(Rt::Tick);
                self.f.op(I::End);
            }
            _ => {}
        }
    }

    /// Pushes whether `a` and `b`, both of type `ty`, are equal: `equal` in `velme_builtins` after its pair is
    /// charged, charging every pair under it as it goes, so it stops where the fuel runs out (D-83).
    fn equal(&mut self, ty: &Ty, a: &[u32], b: &[u32]) -> Result<(), EmitError> {
        match (ty, a, b) {
            // One canonical form per value (D-113), so equal numbers are equal bits.
            (Ty::Number, &[a_lo, a_hi], &[b_lo, b_hi]) => {
                self.f.ops([I::LocalGet(a_lo), I::LocalGet(b_lo), I::I64Eq]);
                self.f.ops([I::LocalGet(a_hi), I::LocalGet(b_hi), I::I64Eq, I::I32And]);
            }
            (Ty::Boolean, &[a], &[b]) => self.f.ops([I::LocalGet(a), I::LocalGet(b), I::I32Eq]),
            (Ty::Nothing, [], []) => self.f.op(I::I32Const(1)),
            (Ty::Text, a, b) => {
                self.f.get(a);
                self.f.get(b);
                self.call(Rt::TextEq);
            }
            (Ty::List(of), a, b) => {
                let helper = self.list_equal(of)?;
                self.f.get(a);
                self.f.get(b);
                self.f.call(helper);
            }
            (Ty::Record(index), a, b) => {
                let helper = self.record_equal(*index)?;
                self.f.get(a);
                self.f.get(b);
                self.f.call(helper);
            }
            // Nothing and a present value differ with nothing more to charge; two present values are their `T`s.
            (Ty::Optional(of), [a_tag, a @ ..], [b_tag, b @ ..]) => {
                self.f
                    .ops([I::LocalGet(*a_tag), I::LocalGet(*b_tag), I::I32Ne, I::If(ONE_I32)]);
                self.f
                    .ops([I::I32Const(0), I::Else, I::LocalGet(*a_tag), I::If(ONE_I32)]);
                self.equal(of, a, b)?;
                self.f.ops([I::Else, I::I32Const(1), I::End, I::End]);
            }
            _ => return Err(internal()),
        }
        Ok(())
    }

    /// Returns 0 from the helper being emitted unless the `i32` on the stack is true.
    fn or_return_false(&mut self) {
        self.f.ops([I::I32Eqz, I::If(Empty), I::I32Const(0), I::Return, I::End]);
    }

    /// The function `(ptr, len, ptr, len) -> i32` comparing two lists of `item`: lists of different lengths differ
    /// once their pair is visited, and each pair of items costs 1 as it is reached.
    fn list_equal(&mut self, item: &Ty) -> Result<u32, EmitError> {
        if let Some(index) = self.list_equals.get(item) {
            return Ok(*index);
        }
        let index = self.assembly.reserve();
        self.list_equals.insert(item.clone(), index);
        let slot = self.types.slot(item)?;
        self.helper(index, &[V::I32; 4], &[V::I32], |e| {
            let [i, at_a, at_b] = [(); 3].map(|()| e.f.local(V::I32));
            e.f.ops([I::LocalGet(1), I::LocalGet(3), I::I32Ne, I::If(Empty)]);
            e.f.ops([I::I32Const(0), I::Return, I::End]);
            e.f.ops([I::I32Const(0), I::LocalSet(i), I::Block(Empty), I::Loop(Empty)]);
            e.f.ops([I::LocalGet(i), I::LocalGet(1), I::I32GeU, I::BrIf(1)]);
            e.call(Rt::Tick);
            for (list, at) in [(0, at_a), (2, at_b)] {
                e.f.ops([I::LocalGet(list), I::LocalGet(i), int(slot), I::I32Mul, I::I32Add]);
                e.f.op(I::LocalSet(at));
            }
            e.load(item, at_a, 0, true)?;
            let a = e.pop(item);
            e.load(item, at_b, 0, true)?;
            let b = e.pop(item);
            e.equal(item, &a, &b)?;
            e.or_return_false();
            e.f.ops([I::LocalGet(i), I::I32Const(1), I::I32Add, I::LocalSet(i)]);
            e.f.ops([I::Br(0), I::End, I::End, I::I32Const(1)]);
            Ok(())
        })?;
        Ok(index)
    }

    /// The function `(ptr, ptr) -> i32` comparing two records of one type, field by field in declared order.
    fn record_equal(&mut self, record: usize) -> Result<u32, EmitError> {
        if let Some(index) = self.record_equals.get(&record) {
            return Ok(*index);
        }
        let index = self.assembly.reserve();
        self.record_equals.insert(record, index);
        let fields = self.types.record(record)?.fields.clone();
        self.helper(index, &[V::I32; 2], &[V::I32], |e| {
            for field in &fields {
                e.call(Rt::Tick);
                e.load(&field.ty, 0, field.offset, true)?;
                let a = e.pop(&field.ty);
                e.load(&field.ty, 1, field.offset, true)?;
                let b = e.pop(&field.ty);
                e.equal(&field.ty, &a, &b)?;
                e.or_return_false();
                e.f.release(&a);
                e.f.release(&b);
            }
            e.f.op(I::I32Const(1));
            Ok(())
        })?;
        Ok(index)
    }

    /// Pushes the flat value of `node`. Each node costs one fuel unit before anything else (R-RUN-04).
    fn expr(&mut self, node: &'ir Node) -> Result<(), EmitError> {
        self.call(Rt::Tick);
        match node {
            Node::Literal { ty, value } => self.literal(ty, value),
            Node::Input { name } => self.input(name),
            Node::Local { name } => self.local(name),
            Node::Record { ty, fields } => self.record(ty, fields),
            Node::List { items, .. } => self.list(node, items),
            Node::FieldGet { of, field } => self.field(of, field),
            Node::BinaryOp { op, left, right } => self.binary(*op, left, right),
            Node::UnaryOp { op, arg } => self.unary(*op, arg),
            Node::Let { bind, body } => self.let_in(bind, body),
            Node::Condition { cond, then, otherwise } => self.condition(node, cond, then, otherwise),
            Node::Narrow { of, default } => self.narrow(node, of, default),
            Node::Map { list, func, .. } => self.map(node, list, func),
            Node::Filter { list, func } => self.filter(list, func),
            Node::Find { list, func } => self.find(node, list, func),
            Node::All { list, func } => self.all_any(list, func, false),
            Node::Any { list, func } => self.all_any(list, func, true),
            Node::Reduce { list, init, func } => self.reduce(list, init, func),
            Node::Sort { list, key, descending } => self.sort(list, key, *descending),
            Node::Builtin { name, args } => self.builtin(name, args),
            Node::Call(_) => Err(internal()),
        }
    }

    /// `Node::Literal`: decoded once, by the validator (D-83), and placed in the data segment; evaluating it
    /// allocates nothing (§7.1).
    fn literal(&mut self, ty: &velme_ir::Type, value: &velme_ir::LiteralValue) -> Result<(), EmitError> {
        let ty = self.types.resolve(ty)?;
        let value = value.decoded().ok_or_else(internal)?;
        for constant in self.data.flat(value, &ty, &self.types)? {
            self.f.op(match constant {
                Const::I32(x) => I::I32Const(x),
                Const::I64(x) => I::I64Const(x),
            });
        }
        Ok(())
    }

    /// `Node::Input`: read from where the host wrote it.
    fn input(&mut self, name: &str) -> Result<(), EmitError> {
        let (_, ty, offset) = self.inputs.iter().find(|(n, ..)| *n == name).ok_or_else(internal)?;
        let (ty, offset) = (ty.clone(), *offset);
        self.load(&ty, INPUTS, offset, true)
    }

    /// `Node::Local`.
    fn local(&mut self, name: &str) -> Result<(), EmitError> {
        let (_, locals) = self.scope.iter().rev().find(|(n, _)| *n == name).ok_or_else(internal)?;
        self.f.get(locals);
        Ok(())
    }

    /// `Node::Record` and `Evaluator::record`: the fields in declaration order (R-RUN-02), then the record's size
    /// charged, then its slot, which starts with that size.
    fn record(&mut self, name: &str, fields: &'ir BTreeMap<String, Node>) -> Result<(), EmitError> {
        let Ty::Record(index) = self.types.resolve(&velme_ir::Type::Record { name: name.to_owned() })? else {
            return Err(internal());
        };
        let record = self.types.record(index)?;
        let (declared, slot) = (record.fields.clone(), record.slot);
        let total = self.f.local(V::I64);
        self.f.ops([size(HEADER_BYTES), I::LocalSet(total)]);
        let mut values = Vec::with_capacity(declared.len());
        for field in &declared {
            let node = fields.get(&field.name).ok_or_else(internal)?;
            self.expr(node)?;
            let found = self.ty(node)?;
            self.coerce(&found, &field.ty)?;
            let vals = self.pop(&field.ty);
            self.add_bytes(total, &field.ty, &vals)?;
            values.push(vals);
        }
        self.f.op(I::LocalGet(total));
        self.call(Rt::ChargeMemory);
        let at = self.f.local(V::I32);
        self.f.op(size(u64::from(slot)));
        self.call(Rt::Bump);
        self.f
            .ops([I::LocalTee(at), I::LocalGet(total), I::I64Store(mem(0, 3))]);
        for (field, vals) in declared.iter().zip(&values) {
            self.store(&field.ty, at, field.offset, vals, true)?;
            self.f.release(vals);
        }
        self.f.op(I::LocalGet(at));
        self.f.release(&[total, at]);
        Ok(())
    }

    /// Allocates a list of `n` items of `slot` bytes whose logical size is in `total`, and returns the local holding
    /// the address of its first item. The size goes before the items ([`LIST_PREFIX`]).
    fn new_list(&mut self, n: I<'static>, slot: u32, total: u32) -> u32 {
        let at = self.f.local(V::I32);
        self.f.ops([n, I::I64ExtendI32U, size(u64::from(slot)), I::I64Mul]);
        self.f.ops([size(u64::from(LIST_PREFIX)), I::I64Add]);
        self.call(Rt::Bump);
        self.f.op(I::LocalSet(at));
        self.f
            .ops([I::LocalGet(at), I::LocalGet(total), I::I64Store(mem(0, 3))]);
        self.f
            .ops([I::LocalGet(at), int(LIST_PREFIX), I::I32Add, I::LocalSet(at)]);
        at
    }

    /// `Node::List`: the items in order, then the list's size charged, then the list.
    fn list(&mut self, node: &Node, items: &'ir [Node]) -> Result<(), EmitError> {
        let Ty::List(of) = self.ty(node)? else {
            return Err(internal());
        };
        let slot = self.types.slot(&of)?;
        let total = self.f.local(V::I64);
        self.f.ops([size(HEADER_BYTES), I::LocalSet(total)]);
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            self.expr(item)?;
            let found = self.ty(item)?;
            self.coerce(&found, &of)?;
            let vals = self.pop(&of);
            self.add_bytes(total, &of, &vals)?;
            values.push(vals);
        }
        self.f.op(I::LocalGet(total));
        self.call(Rt::ChargeMemory);
        let count = u32::try_from(items.len()).map_err(|_| internal())?;
        let at = self.new_list(int(count), slot, total);
        let mut offset = 0u32;
        for vals in &values {
            self.store(&of, at, offset, vals, true)?;
            self.f.release(vals);
            offset = offset.checked_add(slot).ok_or_else(internal)?;
        }
        self.f.ops([I::LocalGet(at), int(count)]);
        self.f.release(&[total, at]);
        Ok(())
    }

    /// `Node::FieldGet`.
    fn field(&mut self, of: &'ir Node, field: &str) -> Result<(), EmitError> {
        let Ty::Record(index) = self.ty(of)? else {
            return Err(internal());
        };
        let found = self.types.record(index)?.fields.iter().find(|f| f.name == field);
        let found = found.ok_or_else(internal)?.clone();
        self.expr(of)?;
        let at = self.f.local(V::I32);
        self.f.op(I::LocalSet(at));
        self.load(&found.ty, at, found.offset, true)?;
        self.f.release(&[at]);
        Ok(())
    }

    /// `Evaluator::binary`.
    fn binary(&mut self, op: BinaryOperator, left: &'ir Node, right: &'ir Node) -> Result<(), EmitError> {
        match op {
            // `and` stops at `false`, `or` at `true`.
            BinaryOperator::And | BinaryOperator::Or => {
                self.expr(left)?;
                self.f.op(I::If(ONE_I32));
                if op == BinaryOperator::Or {
                    self.f.ops([I::I32Const(1), I::Else]);
                    self.expr(right)?;
                } else {
                    self.expr(right)?;
                    self.f.ops([I::Else, I::I32Const(0)]);
                }
                self.f.op(I::End);
            }
            // Metered as it compares (D-83): the interpreter's `equals`.
            BinaryOperator::Eq | BinaryOperator::Ne => {
                let (l, r) = (self.ty(left)?, self.ty(right)?);
                let both = join(&l, &r).filter(|_| assignable(&l, &r) || assignable(&r, &l));
                let both = both.ok_or_else(internal)?;
                self.expr(left)?;
                self.coerce(&l, &both)?;
                let a = self.pop(&both);
                self.expr(right)?;
                self.coerce(&r, &both)?;
                let b = self.pop(&both);
                self.charge_pair(&both, &a, &b);
                self.equal(&both, &a, &b)?;
                if op == BinaryOperator::Ne {
                    self.f.op(I::I32Eqz);
                }
                self.f.release(&a);
                self.f.release(&b);
            }
            // The host's `Number` is the interpreter's (R-SBX-06); an overflow or a division by zero doesn't return.
            BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul | BinaryOperator::Div => {
                self.expr(left)?;
                self.expr(right)?;
                self.import(match op {
                    BinaryOperator::Add => Import::NumAdd,
                    BinaryOperator::Sub => Import::NumSub,
                    BinaryOperator::Mul => Import::NumMul,
                    _ => Import::NumDiv,
                });
            }
            BinaryOperator::Lt | BinaryOperator::Le | BinaryOperator::Gt | BinaryOperator::Ge => {
                self.expr(left)?;
                self.expr(right)?;
                self.import(Import::NumCmp);
                self.f.op(I::I32Const(0));
                self.f.op(match op {
                    BinaryOperator::Lt => I::I32LtS,
                    BinaryOperator::Le => I::I32LeS,
                    BinaryOperator::Gt => I::I32GtS,
                    _ => I::I32GeS,
                });
            }
        }
        Ok(())
    }

    /// `Node::UnaryOp`.
    fn unary(&mut self, op: UnaryOperator, arg: &'ir Node) -> Result<(), EmitError> {
        self.expr(arg)?;
        match op {
            UnaryOperator::Neg => self.import(Import::NumNeg),
            UnaryOperator::Not => self.f.op(I::I32Eqz),
            UnaryOperator::IsEmpty => {
                let ty = self.ty(arg)?;
                self.is_empty(&ty)?;
            }
        }
        Ok(())
    }

    /// `Function::IsEmpty` on the value of type `ty` on the stack: nothing, an empty list or an empty text, and a
    /// present list or text is looked into, as the interpreter's value is the list or text itself (D-60).
    fn is_empty(&mut self, ty: &Ty) -> Result<(), EmitError> {
        let vals = self.pop(ty);
        match (ty, vals.as_slice()) {
            (Ty::Nothing, []) => self.f.op(I::I32Const(1)),
            (Ty::List(_) | Ty::Text, &[_, len]) => self.f.ops([I::LocalGet(len), I::I32Eqz]),
            (Ty::Optional(of), [tag, rest @ ..]) => {
                self.f.ops([I::LocalGet(*tag), I::I32Eqz]);
                if let (Ty::List(_) | Ty::Text, &[_, len]) = (&**of, rest) {
                    self.f.ops([I::LocalGet(len), I::I32Eqz, I::I32Or]);
                }
            }
            _ => self.f.op(I::I32Const(0)),
        }
        self.f.release(&vals);
        Ok(())
    }

    /// `Node::Let`: each binding in order, in scope for the later ones and the body.
    fn let_in(&mut self, bind: &'ir [(String, Node)], body: &'ir Node) -> Result<(), EmitError> {
        let mark = self.scope.len();
        for (name, value) in bind {
            self.expr(value)?;
            let ty = self.ty(value)?;
            let locals = self.pop(&ty);
            self.scope.push((name, locals));
        }
        self.expr(body)?;
        for (_, locals) in self.scope.split_off(mark) {
            self.f.release(&locals);
        }
        Ok(())
    }

    /// `Node::Condition`: only the branch taken is evaluated; both give the type they join in (R-TYP-26).
    fn condition(
        &mut self,
        node: &Node,
        cond: &'ir Node,
        then: &'ir Node,
        otherwise: &'ir Node,
    ) -> Result<(), EmitError> {
        let ty = self.ty(node)?;
        self.expr(cond)?;
        let block = self.assembly.block(&self.types.flat(&ty));
        self.f.op(I::If(block));
        for (i, branch) in [then, otherwise].into_iter().enumerate() {
            if i == 1 {
                self.f.op(I::Else);
            }
            self.expr(branch)?;
            let found = self.ty(branch)?;
            self.coerce(&found, &ty)?;
        }
        self.f.op(I::End);
        Ok(())
    }

    /// `Node::Narrow`: `default` only when it is needed (R-IR-05).
    fn narrow(&mut self, node: &Node, of: &'ir Node, default: &'ir Node) -> Result<(), EmitError> {
        let ty = self.ty(node)?;
        let fallback = self.ty(default)?;
        self.expr(of)?;
        let found = self.ty(of)?;
        if found == Ty::Nothing {
            self.expr(default)?;
            return self.coerce(&fallback, &ty);
        }
        let vals = self.pop(&found);
        let [tag, present @ ..] = vals.as_slice() else {
            return Err(internal());
        };
        let block = self.assembly.block(&self.types.flat(&ty));
        self.f.ops([I::LocalGet(*tag), I::If(block)]);
        self.f.get(present);
        self.f.op(I::Else);
        self.expr(default)?;
        self.coerce(&fallback, &ty)?;
        self.f.op(I::End);
        self.f.release(&vals);
        Ok(())
    }

    /// Evaluates the `list` of a collection node into two locals, its address and its length, and gives its item
    /// type. A node that keeps something per item in its scratch frame is `bounded` by the frame (R-SBX-19).
    fn operand(&mut self, list: &'ir Node, bounded: bool) -> Result<(Ty, u32, u32), EmitError> {
        let Ty::List(item) = self.ty(list)? else {
            return Err(internal());
        };
        self.expr(list)?;
        let (at, n) = (self.f.local(V::I32), self.f.local(V::I32));
        self.f.set(&[at, n]);
        if bounded {
            // No list is longer than `max_list_size` (R-TYP-24), so this is a bug, not a limit.
            self.f.ops([I::LocalGet(n), int(LIST_ITEMS), I::I32GtU, I::If(Empty)]);
            self.trap(Reason::Internal);
            self.f.op(I::End);
        }
        Ok((*item, at, n))
    }

    /// Opens the loop of a collection node over the `n` items of type `item` at `at`, up to where its lambda runs
    /// with `param` bound: `Evaluator::visit`. The element's unit is charged first; from then until
    /// [`Emitter::visited`] the frame says which element a failure happened at (R-BLT-07). Inside, `br 0` is the next
    /// element and `br 1` leaves the loop.
    fn visit(&mut self, item: &Ty, at: u32, n: u32, param: &'ir str) -> Result<Loop, EmitError> {
        if self.level + 1 >= FRAMES {
            return Err(internal());
        }
        let slot = self.types.slot(item)?;
        let (i, element) = (self.f.local(V::I32), self.f.local(V::I32));
        self.f
            .ops([I::I32Const(0), I::LocalSet(i), I::Block(Empty), I::Loop(Empty)]);
        self.f.ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
        self.call(Rt::Tick);
        self.f
            .ops([I::LocalGet(at), I::LocalGet(i), int(slot), I::I32Mul, I::I32Add]);
        self.f.op(I::LocalSet(element));
        self.load(item, element, 0, true)?;
        let locals = self.pop(item);
        self.f.release(&[element]);
        self.f.ops([
            int(frame(self.level) + FRAME_VISITING),
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
        ]);
        self.f.op(I::I32Store(mem(0, 2)));
        let mark = self.scope.len();
        self.scope.push((param, locals.clone()));
        self.level += 1;
        Ok(Loop { i, param: locals, mark })
    }

    /// The lambda has run: a failure from here on is the node's own, not the element's. The lambda's value stays
    /// on the stack.
    fn visited(&mut self, open: &Loop) {
        self.level -= 1;
        self.scope.truncate(open.mark);
        self.f.ops([
            int(frame(self.level) + FRAME_VISITING),
            I::I32Const(0),
            I::I32Store(mem(0, 2)),
        ]);
    }

    /// Closes the loop [`Emitter::visit`] opened.
    fn next(&mut self, open: Loop) {
        self.f
            .ops([I::LocalGet(open.i), I::I32Const(1), I::I32Add, I::LocalSet(open.i)]);
        self.f.ops([I::Br(0), I::End, I::End]);
        self.f.release(&open.param);
        self.f.release(&[open.i]);
    }

    /// The address of this level's collection area.
    fn area(&self) -> u32 {
        frame(self.level) + FRAME_AREA
    }

    /// Sets `cell` to the address of cell `i` of `bytes` bytes, `offset` into this level's collection area.
    fn cell(&mut self, cell: u32, offset: u32, i: u32, bytes: u32) {
        self.f.ops([
            int(self.area() + offset),
            I::LocalGet(i),
            int(bytes),
            I::I32Mul,
            I::I32Add,
        ]);
        self.f.op(I::LocalSet(cell));
    }

    /// `Node::Map`: each result is kept in the scratch frame until every element is visited; then the list is
    /// charged, and only then takes memory (§7.1, R-SBX-19).
    fn map(&mut self, node: &Node, list: &'ir Node, func: &'ir Lambda) -> Result<(), EmitError> {
        let Ty::List(result) = self.ty(node)? else {
            return Err(internal());
        };
        let slot = self.types.slot(&result)?;
        let (item, at, n) = self.operand(list, true)?;
        let (total, cell) = (self.f.local(V::I64), self.f.local(V::I32));
        self.f.ops([size(HEADER_BYTES), I::LocalSet(total)]);
        let open = self.visit(&item, at, n, &func.param)?;
        self.expr(&func.body)?;
        self.visited(&open);
        let vals = self.pop(&result);
        self.add_bytes(total, &result, &vals)?;
        self.cell(cell, 0, open.i, AREA_ITEM_BYTES);
        self.store(&result, cell, 0, &vals, false)?;
        self.f.release(&vals);
        self.next(open);
        self.f.op(I::LocalGet(total));
        self.call(Rt::ChargeMemory);
        let out = self.new_list(I::LocalGet(n), slot, total);
        let (i, to) = (self.f.local(V::I32), self.f.local(V::I32));
        self.f
            .ops([I::I32Const(0), I::LocalSet(i), I::Block(Empty), I::Loop(Empty)]);
        self.f.ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
        self.cell(cell, 0, i, AREA_ITEM_BYTES);
        self.f.ops([
            I::LocalGet(out),
            I::LocalGet(i),
            int(slot),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(to),
        ]);
        self.load(&result, cell, 0, false)?;
        let vals = self.pop(&result);
        self.store(&result, to, 0, &vals, true)?;
        self.f.release(&vals);
        self.f.ops([I::LocalGet(i), I::I32Const(1), I::I32Add, I::LocalSet(i)]);
        self.f.ops([I::Br(0), I::End, I::End]);
        self.f.ops([I::LocalGet(out), I::LocalGet(n)]);
        self.f.release(&[at, n, total, cell, out, i, to]);
        Ok(())
    }

    /// Copies `count` slots of `slot` bytes into a new list whose logical size is in `total`, the `j`th from the
    /// slot of `at` whose index is the `j`th `i32` at `indexes` in this level's collection area; pushes the list.
    fn gather(&mut self, at: u32, slot: u32, count: u32, indexes: u32, total: u32) {
        let out = self.new_list(I::LocalGet(count), slot, total);
        let (j, from) = (self.f.local(V::I32), self.f.local(V::I32));
        self.f
            .ops([I::I32Const(0), I::LocalSet(j), I::Block(Empty), I::Loop(Empty)]);
        self.f.ops([I::LocalGet(j), I::LocalGet(count), I::I32GeU, I::BrIf(1)]);
        self.cell(from, indexes, j, 4);
        self.f
            .ops([I::LocalGet(out), I::LocalGet(j), int(slot), I::I32Mul, I::I32Add]);
        self.f.ops([
            I::LocalGet(at),
            I::LocalGet(from),
            I::I32Load(mem(0, 2)),
            int(slot),
            I::I32Mul,
            I::I32Add,
        ]);
        self.f.ops([int(slot), COPY]);
        self.f.ops([I::LocalGet(j), I::I32Const(1), I::I32Add, I::LocalSet(j)]);
        self.f.ops([I::Br(0), I::End, I::End]);
        self.f.ops([I::LocalGet(out), I::LocalGet(count)]);
        self.f.release(&[out, j, from]);
    }

    /// `Node::Filter`: the indexes kept are in the scratch frame until every element is visited; then the list is
    /// charged and made.
    fn filter(&mut self, list: &'ir Node, func: &'ir Lambda) -> Result<(), EmitError> {
        let (item, at, n) = self.operand(list, true)?;
        let slot = self.types.slot(&item)?;
        let (total, kept, cell) = (self.f.local(V::I64), self.f.local(V::I32), self.f.local(V::I32));
        self.f.ops([
            size(HEADER_BYTES),
            I::LocalSet(total),
            I::I32Const(0),
            I::LocalSet(kept),
        ]);
        let open = self.visit(&item, at, n, &func.param)?;
        self.expr(&func.body)?;
        self.visited(&open);
        self.f.op(I::If(Empty));
        self.cell(cell, 0, kept, 4);
        self.f
            .ops([I::LocalGet(cell), I::LocalGet(open.i), I::I32Store(mem(0, 2))]);
        self.f
            .ops([I::LocalGet(kept), I::I32Const(1), I::I32Add, I::LocalSet(kept)]);
        self.add_bytes(total, &item, &open.param)?;
        self.f.op(I::End);
        self.next(open);
        self.f.op(I::LocalGet(total));
        self.call(Rt::ChargeMemory);
        self.gather(at, slot, kept, 0, total);
        self.f.release(&[at, n, total, kept, cell]);
        Ok(())
    }

    /// `Node::Find`: the first element the predicate holds for, which is no new value, or nothing.
    fn find(&mut self, node: &Node, list: &'ir Node, func: &'ir Lambda) -> Result<(), EmitError> {
        let ty = self.ty(node)?;
        let (item, at, n) = self.operand(list, false)?;
        self.zeros(&ty);
        let found = self.pop(&ty);
        let open = self.visit(&item, at, n, &func.param)?;
        self.expr(&func.body)?;
        self.visited(&open);
        self.f.op(I::If(Empty));
        self.f.get(&open.param);
        self.coerce(&item, &ty)?;
        self.f.set(&found);
        self.f.ops([I::Br(2), I::End]);
        self.next(open);
        self.f.get(&found);
        self.f.release(&found);
        self.f.release(&[at, n]);
        Ok(())
    }

    /// `Node::All` and `Node::Any`: they stop at the first element that decides the answer (D-58).
    fn all_any(&mut self, list: &'ir Node, func: &'ir Lambda, any: bool) -> Result<(), EmitError> {
        let (item, at, n) = self.operand(list, false)?;
        let answer = self.f.local(V::I32);
        self.f.ops([I::I32Const(i32::from(!any)), I::LocalSet(answer)]);
        let open = self.visit(&item, at, n, &func.param)?;
        self.expr(&func.body)?;
        self.visited(&open);
        if !any {
            self.f.op(I::I32Eqz);
        }
        self.f.ops([
            I::If(Empty),
            I::I32Const(i32::from(any)),
            I::LocalSet(answer),
            I::Br(2),
            I::End,
        ]);
        self.next(open);
        self.f.op(I::LocalGet(answer));
        self.f.release(&[at, n, answer]);
        Ok(())
    }

    /// `Evaluator::reduce`: the list first, then `init` (R-RUN-02), then the body once per element.
    fn reduce(&mut self, list: &'ir Node, init: &'ir Node, func: &'ir ReduceLambda) -> Result<(), EmitError> {
        let (item, at, n) = self.operand(list, false)?;
        self.expr(init)?;
        let ty = self.ty(init)?;
        let acc = self.pop(&ty);
        let mark = self.scope.len();
        self.scope.push((&func.acc, acc.clone()));
        let open = self.visit(&item, at, n, &func.param)?;
        self.expr(&func.body)?;
        let found = self.ty(&func.body)?;
        self.coerce(&found, &ty)?;
        self.visited(&open);
        self.f.set(&acc);
        self.next(open);
        self.scope.truncate(mark);
        self.f.get(&acc);
        self.f.release(&acc);
        self.f.release(&[at, n]);
        Ok(())
    }

    /// `Evaluator::sort`: every key first, in list order; then the list's size is charged, then the rest of the
    /// fuel (D-88), both before the work; then a stable sort (R-BLT-11).
    fn sort(&mut self, list: &'ir Node, key: &'ir Lambda, descending: bool) -> Result<(), EmitError> {
        let (item, at, n) = self.operand(list, true)?;
        let slot = self.types.slot(&item)?;
        let (cell, total) = (self.f.local(V::I32), self.f.local(V::I64));
        let open = self.visit(&item, at, n, &key.param)?;
        self.expr(&key.body)?;
        self.visited(&open);
        let vals = self.pop(&Ty::Number);
        self.cell(cell, 0, open.i, 16);
        self.store(&Ty::Number, cell, 0, &vals, true)?;
        self.f.release(&vals);
        self.next(open);
        // The sorted list is as large as the list, which knows its size.
        self.f
            .ops([I::LocalGet(at), int(LIST_PREFIX), I::I32Sub, I::I64Load(mem(0, 3))]);
        self.f.ops([I::LocalTee(total)]);
        self.call(Rt::ChargeMemory);
        // `sort_by_fuel(n)` less its own unit and the one per key, already charged: n·⌈log2(n+1)⌉ (D-52).
        self.f.ops([I::LocalGet(n), I::I64ExtendI32U, I::I64Const(64)]);
        self.f
            .ops([I::LocalGet(n), I::I64ExtendI32U, I::I64Clz, I::I64Sub, I::I64Mul]);
        self.call(Rt::ChargeFuel);
        self.f
            .ops([int(self.area()), I::LocalGet(n), I::I32Const(i32::from(descending))]);
        self.call(Rt::Sort);
        self.gather(at, slot, n, SORT_ORDER, total);
        self.f.release(&[at, n, cell, total]);
        Ok(())
    }

    /// `Node::Builtin`: the arguments in order, then the call, which costs what `Function::call` says and is charged
    /// as `Evaluator::settle` charges it; this node's unit is the catalog cost's first (D-52).
    fn builtin(&mut self, name: &str, args: &'ir [Node]) -> Result<(), EmitError> {
        let function = Builtin::find(name).and_then(|b| b.function).ok_or_else(internal)?;
        if function == Function::Contains {
            return self.contains(args);
        }
        for arg in args {
            self.expr(arg)?;
        }
        let first = args.first().map(|arg| self.ty(arg)).transpose()?;
        match function {
            Function::Length => match first {
                Some(Ty::List(_)) => {
                    let len = self.f.local(V::I32);
                    self.f.ops([
                        I::LocalSet(len),
                        I::Drop,
                        I::LocalGet(len),
                        I::I64ExtendI32U,
                        I::I64Const(0),
                    ]);
                    self.f.release(&[len]);
                }
                // A text's length is its characters, and costs its blocks before it is counted.
                Some(Ty::Text) => {
                    let (at, len) = (self.f.local(V::I32), self.f.local(V::I32));
                    self.f.set(&[at, len]);
                    self.f.ops([I::LocalGet(len), I::I64ExtendI32U]);
                    self.call(Rt::Blocks);
                    self.call(Rt::ChargeFuel);
                    self.f.get(&[at, len]);
                    self.call(Rt::TextChars);
                    self.f.ops([I::I64ExtendI32U, I::I64Const(0)]);
                    self.f.release(&[at, len]);
                }
                _ => return Err(internal()),
            },
            Function::IsEmpty => self.is_empty(&first.ok_or_else(internal)?)?,
            Function::Maximum | Function::Minimum => {
                self.f
                    .op(I::I32Const(if function == Function::Maximum { 1 } else { -1 }));
                self.call(Rt::Extreme);
            }
            Function::Sum => self.call(Rt::Sum),
            Function::Abs => self.import(Import::Abs),
            Function::Floor => self.import(Import::Floor),
            Function::Ceil => self.import(Import::Ceil),
            Function::Round => self.import(Import::Round),
            Function::Clamp => self.import(Import::Clamp),
            Function::Random => self.import(Import::Random),
            Function::Concat => self.call(Rt::Concat),
            Function::ToText => {
                self.f.op(int(frame(self.level) + FRAME_MARSHAL));
                self.call(Rt::ToText);
            }
            Function::Range => self.call(Rt::Range),
            Function::Contains => return Err(internal()),
        }
        Ok(())
    }

    /// `Function::Contains`: each item scanned costs its unit and what `item == wanted` costs, up to and including
    /// the match (D-83).
    fn contains(&mut self, args: &'ir [Node]) -> Result<(), EmitError> {
        let [list, wanted] = args else {
            return Err(internal());
        };
        let (item, at, n) = self.operand(list, false)?;
        let found = self.ty(wanted)?;
        let both = join(&item, &found).ok_or_else(internal)?;
        let slot = self.types.slot(&item)?;
        self.expr(wanted)?;
        self.coerce(&found, &both)?;
        let b = self.pop(&both);
        let (answer, i, element) = (self.f.local(V::I32), self.f.local(V::I32), self.f.local(V::I32));
        self.f
            .ops([I::I32Const(0), I::LocalSet(answer), I::I32Const(0), I::LocalSet(i)]);
        self.f.ops([I::Block(Empty), I::Loop(Empty)]);
        self.f.ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
        self.call(Rt::Tick);
        self.f
            .ops([I::LocalGet(at), I::LocalGet(i), int(slot), I::I32Mul, I::I32Add]);
        self.f.op(I::LocalSet(element));
        self.load(&item, element, 0, true)?;
        self.coerce(&item, &both)?;
        let a = self.pop(&both);
        self.charge_pair(&both, &a, &b);
        self.equal(&both, &a, &b)?;
        self.f
            .ops([I::If(Empty), I::I32Const(1), I::LocalSet(answer), I::Br(2), I::End]);
        self.f.release(&a);
        self.f.ops([I::LocalGet(i), I::I32Const(1), I::I32Add, I::LocalSet(i)]);
        self.f.ops([I::Br(0), I::End, I::End]);
        self.f.op(I::LocalGet(answer));
        self.f.release(&b);
        self.f.release(&[at, n, answer, i, element]);
        Ok(())
    }
}
