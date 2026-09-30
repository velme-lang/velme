//! Assembling a module with `wasm-encoder` (`runtime/31` §2): functions built as instruction lists with pooled
//! locals, interned function types, and the fixed sections of R-SBX-03.

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    GlobalSection, GlobalType, ImportSection, Instruction, MemorySection, MemoryType, TypeSection, ValType,
};

use crate::abi::{self, Import};

/// A value type of emitted code: there are no floats in a module (R-SBX-07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum V {
    I32,
    I64,
}

impl V {
    fn val(self) -> ValType {
        match self {
            V::I32 => ValType::I32,
            V::I64 => ValType::I64,
        }
    }
}

/// The exported fuel global.
pub(crate) const G_FUEL: u32 = 0;
/// The exported memory global.
pub(crate) const G_MEMORY: u32 = 1;
/// The exported reason global.
pub(crate) const G_REASON: u32 = 2;
/// The bump allocator's next free address; not exported.
pub(crate) const G_HEAP: u32 = 3;

/// A memory operand at `offset` from the address on the stack, aligned for a value of `2^align` bytes.
pub(crate) fn mem(offset: u32, align: u32) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset: u64::from(offset),
        align,
        memory_index: 0,
    }
}

/// A function being emitted: its instructions, and its locals, which are reused once released so a large body
/// stays within a validator's local limit.
#[derive(Debug)]
pub(crate) struct Func {
    params: Vec<V>,
    results: Vec<V>,
    /// The types of the locals after the parameters.
    locals: Vec<V>,
    /// Released locals, by type.
    free: [Vec<u32>; 2],
    code: Vec<Instruction<'static>>,
}

impl Func {
    pub(crate) fn new(params: &[V], results: &[V]) -> Func {
        Func {
            params: params.to_vec(),
            results: results.to_vec(),
            locals: Vec::new(),
            free: [Vec::new(), Vec::new()],
            code: Vec::new(),
        }
    }

    fn pool(&mut self, v: V) -> &mut Vec<u32> {
        match v {
            V::I32 => &mut self.free[0],
            V::I64 => &mut self.free[1],
        }
    }

    /// A local of type `v` that nothing else is using. It holds whatever its last user left, so it is set before it
    /// is read.
    pub(crate) fn local(&mut self, v: V) -> u32 {
        if let Some(index) = self.pool(v).pop() {
            return index;
        }
        let index = self.params.len() + self.locals.len();
        self.locals.push(v);
        u32::try_from(index).unwrap_or(u32::MAX)
    }

    /// Gives back locals whose values are dead.
    pub(crate) fn release(&mut self, locals: &[u32]) {
        for &index in locals {
            let own = (index as usize).checked_sub(self.params.len());
            if let Some(v) = own.and_then(|i| self.locals.get(i).copied()) {
                // Released twice, a local would be handed to two users at once.
                debug_assert!(!self.pool(v).contains(&index));
                self.pool(v).push(index);
            }
        }
    }

    pub(crate) fn op(&mut self, instruction: Instruction<'static>) {
        self.code.push(instruction);
    }

    pub(crate) fn ops<const N: usize>(&mut self, instructions: [Instruction<'static>; N]) {
        self.code.extend(instructions);
    }

    /// Pushes the locals `locals`, in order.
    pub(crate) fn get(&mut self, locals: &[u32]) {
        self.code.extend(locals.iter().map(|&l| Instruction::LocalGet(l)));
    }

    /// Pops the values on the stack into `locals`, the last local from the top.
    pub(crate) fn set(&mut self, locals: &[u32]) {
        self.code.extend(locals.iter().rev().map(|&l| Instruction::LocalSet(l)));
    }

    /// Calls the runtime function or import at `index`.
    pub(crate) fn call(&mut self, index: u32) {
        self.code.push(Instruction::Call(index));
    }

    /// The instructions of the body so far: the most it runs between two turns of a loop in it, since each costs
    /// Wasmtime at most one unit of its fuel (`runtime/31` §6, D-115).
    #[cfg(test)]
    pub(crate) fn instructions(&self) -> u64 {
        self.code.len() as u64
    }
}

/// The module being assembled.
#[derive(Debug, Default)]
pub(crate) struct Assembly {
    /// Function types, in the order first used.
    types: Vec<(Vec<V>, Vec<V>)>,
    /// The functions defined, in index order after the imports; `None` while one is still being emitted.
    funcs: Vec<Option<Func>>,
}

impl Assembly {
    fn type_index(&mut self, params: &[V], results: &[V]) -> u32 {
        let found = self.types.iter().position(|(p, r)| p == params && r == results);
        let index = found.unwrap_or_else(|| {
            self.types.push((params.to_vec(), results.to_vec()));
            self.types.len() - 1
        });
        u32::try_from(index).unwrap_or(u32::MAX)
    }

    /// The type of a block leaving `results` on the stack (multi-value, R-SBX-07).
    pub(crate) fn block(&mut self, results: &[V]) -> BlockType {
        match results {
            [] => BlockType::Empty,
            [one] => BlockType::Result(one.val()),
            many => BlockType::FunctionType(self.type_index(&[], many)),
        }
    }

    /// The index of a function to be defined later, so its callers, itself included, can be emitted first.
    pub(crate) fn reserve(&mut self) -> u32 {
        self.funcs.push(None);
        let index = Import::ALL.len() + self.funcs.len() - 1;
        u32::try_from(index).unwrap_or(u32::MAX)
    }

    /// Defines the function reserved at `index`.
    pub(crate) fn define(&mut self, index: u32, func: Func) {
        let at = (index as usize).checked_sub(Import::ALL.len());
        if let Some(slot) = at.and_then(|at| self.funcs.get_mut(at)) {
            *slot = Some(func);
        }
    }

    /// Defines the next function.
    pub(crate) fn push(&mut self, func: Func) -> u32 {
        let index = self.reserve();
        self.define(index, func);
        index
    }

    /// The module's bytes: `data` is placed at [`abi::SCRATCH_BYTES`] and the heap starts after it. `alloc` and `run`
    /// are the indexes of the two exported functions. `None` if a reserved function was never defined.
    pub(crate) fn finish(mut self, data: &[u8], alloc: u32, run: u32) -> Option<Vec<u8>> {
        let funcs = std::mem::take(&mut self.funcs)
            .into_iter()
            .collect::<Option<Vec<Func>>>()?;
        let imports: Vec<u32> = Import::ALL
            .iter()
            .map(|import| {
                let (params, results) = import.signature();
                self.type_index(params, results)
            })
            .collect();
        let defined: Vec<u32> = funcs.iter().map(|f| self.type_index(&f.params, &f.results)).collect();

        let mut module = wasm_encoder::Module::new();
        let mut types = TypeSection::new();
        for (params, results) in &self.types {
            types
                .ty()
                .function(params.iter().map(|v| v.val()), results.iter().map(|v| v.val()));
        }
        module.section(&types);

        let mut section = ImportSection::new();
        for (import, ty) in Import::ALL.iter().zip(imports) {
            section.import(abi::IMPORT_MODULE, import.name(), EntityType::Function(ty));
        }
        module.section(&section);

        let mut section = FunctionSection::new();
        for ty in defined {
            section.function(ty);
        }
        module.section(&section);

        let heap = u64::from(abi::SCRATCH_BYTES) + data.len().next_multiple_of(8) as u64;
        let mut memories = MemorySection::new();
        // No maximum: a run's limits are the host's, never the module's (R-SBX-03).
        memories.memory(MemoryType {
            minimum: heap.div_ceil(PAGE_BYTES),
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        let mut globals = GlobalSection::new();
        let global = |val_type| GlobalType {
            val_type,
            mutable: true,
            shared: false,
        };
        globals.global(global(ValType::I64), &ConstExpr::i64_const(0));
        globals.global(global(ValType::I64), &ConstExpr::i64_const(0));
        globals.global(global(ValType::I32), &ConstExpr::i32_const(0));
        // Below 2^31 for any module a validator accepts, so the cast keeps its value.
        globals.global(global(ValType::I32), &ConstExpr::i32_const(heap as i32));
        module.section(&globals);

        let mut exports = ExportSection::new();
        exports.export(abi::MEMORY, ExportKind::Memory, 0);
        exports.export(abi::ALLOC, ExportKind::Func, alloc);
        exports.export(abi::RUN, ExportKind::Func, run);
        exports.export(abi::FUEL_LEFT, ExportKind::Global, G_FUEL);
        exports.export(abi::MEMORY_LEFT, ExportKind::Global, G_MEMORY);
        exports.export(abi::REASON, ExportKind::Global, G_REASON);
        module.section(&exports);

        let mut code = CodeSection::new();
        for func in funcs {
            let mut body = Function::new_with_locals_types(func.locals.iter().map(|v| v.val()));
            for instruction in &func.code {
                body.instruction(instruction);
            }
            body.instruction(&Instruction::End);
            code.function(&body);
        }
        module.section(&code);

        if !data.is_empty() {
            let mut section = DataSection::new();
            // Below 2^31, so the cast keeps its value.
            section.active(
                0,
                &ConstExpr::i32_const(abi::SCRATCH_BYTES as i32),
                data.iter().copied(),
            );
            module.section(&section);
        }
        Some(module.finish())
    }
}

/// Bytes in a page of linear memory.
pub(crate) const PAGE_BYTES: u64 = 65_536;
