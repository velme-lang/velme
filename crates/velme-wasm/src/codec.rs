//! The host's side of the value layout (`runtime/31` §3): an invocation's inputs as the bytes the module reads, and
//! its result read back out of the module's memory. The layouts are the emitter's own ([`Signature`]). What is read
//! back is checked as if the module were hostile (R-SBX-04, R-SBX-11): every address, length, size and `Number`.

use std::collections::BTreeMap;

use velme_builtins::limits::MAX_LIST_SIZE;
use velme_builtins::memory::{BOOLEAN_BYTES, HEADER_BYTES, NUMBER_BYTES, OPTIONAL_BYTES, text_bytes, value_bytes};
use velme_builtins::{Number, Value};

use crate::abi::{LIST_PREFIX, RESULT};
use crate::ty::{Signature, Ty, Types};

/// What the host found is not a value of the layout: `VL0607`, a backend bug (R-SBX-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Corrupt;

/// The inputs of one invocation, laid out from address 0: the host asks `velme_alloc` for room once, and every
/// address in the image then moves by where that room starts.
#[derive(Debug)]
pub(crate) struct Image {
    bytes: Vec<u8>,
    /// Where the image holds an address.
    addresses: Vec<usize>,
    /// The logical size of the inputs (`runtime/30` §7.1).
    logical: u64,
    /// The items of the inputs' lists of `Nothing`, which have no size.
    sizeless: u64,
}

impl Image {
    /// `inputs`, one per input of `signature`, each in its slot in declared order, with what they point at after
    /// them. Every list and record has its size prefix (D-119).
    pub(crate) fn new(signature: &Signature, inputs: &[Value]) -> Result<Image, Corrupt> {
        if inputs.len() != signature.inputs.len() {
            return Err(Corrupt);
        }
        let mut writer = Writer {
            types: &signature.types,
            image: Image {
                bytes: vec![0; signature.input_bytes as usize],
                addresses: Vec::new(),
                logical: 0,
                sizeless: 0,
            },
        };
        for ((ty, at), value) in signature.inputs.iter().zip(inputs) {
            writer.image.logical = writer.image.logical.saturating_add(value_bytes(value));
            writer.slot(*at as usize, value, ty)?;
        }
        // An address is an `i32` in emitted code.
        i32::try_from(writer.image.bytes.len()).map_err(|_| Corrupt)?;
        Ok(writer.image)
    }

    /// The bytes `velme_alloc` is asked for.
    pub(crate) fn len(&self) -> u32 {
        // `new` has checked that it fits.
        u32::try_from(self.bytes.len()).unwrap_or(u32::MAX)
    }

    /// The logical size of the inputs, which nobody was charged for in this invocation (D-53).
    pub(crate) fn logical(&self) -> u64 {
        self.logical
    }

    /// The items of the inputs' lists of `Nothing`, which nobody paid fuel for in this invocation.
    pub(crate) fn sizeless(&self) -> u64 {
        self.sizeless
    }

    /// The bytes to write at `base`, where the module's memory has room for them.
    pub(crate) fn placed(mut self, base: u32) -> Result<Vec<u8>, Corrupt> {
        for at in std::mem::take(&mut self.addresses) {
            let cell = self.bytes.get_mut(at..at.saturating_add(4)).ok_or(Corrupt)?;
            let relative = u32::from_le_bytes(<[u8; 4]>::try_from(&*cell).map_err(|_| Corrupt)?);
            let address = relative.checked_add(base).ok_or(Corrupt)?;
            cell.copy_from_slice(&address.to_le_bytes());
        }
        Ok(self.bytes)
    }
}

struct Writer<'a> {
    types: &'a Types,
    image: Image,
}

impl Writer<'_> {
    fn put(&mut self, at: usize, bytes: &[u8]) -> Result<(), Corrupt> {
        let cell = self.image.bytes.get_mut(at..at.saturating_add(bytes.len()));
        cell.ok_or(Corrupt)?.copy_from_slice(bytes);
        Ok(())
    }

    /// Room for `size` more bytes at the next 8-aligned address, which it returns.
    fn room(&mut self, size: usize) -> Result<usize, Corrupt> {
        let at = self.image.bytes.len().next_multiple_of(8);
        let end = at.checked_add(size).filter(|end| i32::try_from(*end).is_ok());
        self.image.bytes.resize(end.ok_or(Corrupt)?.next_multiple_of(8), 0);
        Ok(at)
    }

    /// Writes the address `to` at `at`.
    fn address(&mut self, at: usize, to: usize) -> Result<(), Corrupt> {
        self.put(at, &u32::try_from(to).map_err(|_| Corrupt)?.to_le_bytes())?;
        self.image.addresses.push(at);
        Ok(())
    }

    /// Writes `value`, of type `ty`, in its slot at `at` (§3).
    fn slot(&mut self, at: usize, value: &Value, ty: &Ty) -> Result<(), Corrupt> {
        match (ty, value) {
            (Ty::Number, Value::Number(n)) => {
                let (lo, hi) = n.to_bits();
                self.put(at, &lo.to_le_bytes())?;
                self.put(at + 8, &hi.to_le_bytes())
            }
            (Ty::Boolean, Value::Boolean(b)) => self.put(at, &u32::from(*b).to_le_bytes()),
            (Ty::Nothing, Value::Nothing) => Ok(()),
            (Ty::Text, Value::Text(text)) => {
                let to = self.room(text.len())?;
                self.put(to, text.as_bytes())?;
                self.address(at, to)?;
                self.put(at + 4, &u32::try_from(text.len()).map_err(|_| Corrupt)?.to_le_bytes())
            }
            (Ty::List(of), Value::List(list)) => {
                let len = u32::try_from(list.len())
                    .ok()
                    .filter(|n| u64::from(*n) <= MAX_LIST_SIZE);
                let len = len.ok_or(Corrupt)?;
                let slot = self.types.slot(of).map_err(|_| Corrupt)? as usize;
                let prefix = LIST_PREFIX as usize;
                let items = slot.checked_mul(list.len()).and_then(|items| items.checked_add(prefix));
                let to = self.room(items.ok_or(Corrupt)?)?;
                self.put(to, &list.bytes().to_le_bytes())?;
                // Items without a slot are `Nothing`, and the list is its length.
                if slot == 0 {
                    self.image.sizeless = self.image.sizeless.saturating_add(u64::from(len));
                } else {
                    for (i, item) in list.iter().enumerate() {
                        self.slot(to + prefix + i * slot, item, of)?;
                    }
                }
                self.address(at, to + prefix)?;
                self.put(at + 4, &len.to_le_bytes())
            }
            // Zero is nothing: the image starts zeroed.
            (Ty::Optional(_), Value::Nothing) => Ok(()),
            (Ty::Optional(of), present) => {
                let to = self.room(self.types.slot(of).map_err(|_| Corrupt)? as usize)?;
                self.slot(to, present, of)?;
                self.address(at, to)
            }
            (Ty::Record(index), Value::Record(record)) => {
                let layout = self.types.record(*index).map_err(|_| Corrupt)?;
                if Some(record.name.as_str()) != self.types.name(*index) || record.fields.len() != layout.fields.len() {
                    return Err(Corrupt);
                }
                self.put(at, &record.bytes().to_le_bytes())?;
                for (field, (name, value)) in layout.fields.iter().zip(&record.fields) {
                    if *name != field.name {
                        return Err(Corrupt);
                    }
                    self.slot(at + field.offset as usize, value, &field.ty)?;
                }
                Ok(())
            }
            _ => Err(Corrupt),
        }
    }
}

/// The items of the lists in `value` that are `nothing`: at least the items of its lists of `Nothing`, which have no
/// size (R-SBX-11). For a literal, which the validator decoded from JSON, so nothing in it is shared.
pub(crate) fn nothings(value: &Value) -> u64 {
    match value {
        Value::List(list) => list.iter().fold(0u64, |n, item| {
            n.saturating_add(match item {
                Value::Nothing => 1,
                item => nothings(item),
            })
        }),
        Value::Record(record) => record
            .fields
            .iter()
            .fold(0, |n, (_, field)| n.saturating_add(nothings(field))),
        _ => 0,
    }
}

/// The result of a run, read out of the module's memory.
pub(crate) struct Reader<'a> {
    memory: &'a [u8],
    types: &'a Types,
    /// The logical bytes still to decode before the memory is taken to be corrupt.
    bytes: u64,
    /// The items of lists of `Nothing` still to make before the memory is taken to be corrupt.
    items: u64,
    /// The lists of `Nothing` read so far, by length: equal values, so one of each is made and then shared.
    nothings: BTreeMap<u32, Value>,
}

impl<'a> Reader<'a> {
    /// A reader that stops once it has read `bytes` logical bytes, or made `items` items of lists of `Nothing`
    /// (R-SBX-11, D-120): what bounds the host's work whatever the memory holds. Every other value has a size, and
    /// each such item a run made cost it a unit of fuel.
    pub(crate) fn new(memory: &'a [u8], types: &'a Types, bytes: u64, items: u64) -> Reader<'a> {
        Reader {
            memory,
            types,
            bytes,
            items,
            nothings: BTreeMap::new(),
        }
    }

    /// The output of type `ty` that `velme_run` returned `at` for: a record at its own address, anything else in
    /// the result slot (R-SBX-03).
    pub(crate) fn output(&mut self, ty: &Ty, at: u32) -> Result<Value, Corrupt> {
        match ty {
            Ty::Record(_) => self.slot(ty, at),
            _ if at == RESULT => self.slot(ty, at),
            _ => Err(Corrupt),
        }
    }

    fn bytes(&self, at: u32, len: u32) -> Result<&'a [u8], Corrupt> {
        let end = (at as usize).checked_add(len as usize).ok_or(Corrupt)?;
        self.memory.get(at as usize..end).ok_or(Corrupt)
    }

    fn u32_at(&self, at: u32) -> Result<u32, Corrupt> {
        let bytes = <[u8; 4]>::try_from(self.bytes(at, 4)?).map_err(|_| Corrupt)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64_at(&self, at: u32) -> Result<u64, Corrupt> {
        let bytes = <[u8; 8]>::try_from(self.bytes(at, 8)?).map_err(|_| Corrupt)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn take(&mut self, bytes: u64) -> Result<(), Corrupt> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(Corrupt)?;
        Ok(())
    }

    /// The value of type `ty` in the slot at `at`. The depth of the recursion is the depth of `ty`: a type never
    /// holds itself (`language/11` R-TYP-18), whatever the memory's addresses say.
    fn slot(&mut self, ty: &Ty, at: u32) -> Result<Value, Corrupt> {
        match ty {
            Ty::Number => {
                self.take(NUMBER_BYTES)?;
                let hi = self.u64_at(at.checked_add(8).ok_or(Corrupt)?)?;
                Number::from_bits(self.u64_at(at)?, hi)
                    .map(Value::Number)
                    .map_err(|_| Corrupt)
            }
            Ty::Boolean => {
                self.take(BOOLEAN_BYTES)?;
                match self.u32_at(at)? {
                    0 => Ok(Value::Boolean(false)),
                    1 => Ok(Value::Boolean(true)),
                    _ => Err(Corrupt),
                }
            }
            Ty::Nothing => Ok(Value::Nothing),
            Ty::Text => {
                let (ptr, len) = self.pair(at)?;
                self.take(text_bytes(len as usize))?;
                let text = std::str::from_utf8(self.bytes(ptr, len)?).map_err(|_| Corrupt)?;
                Ok(Value::text(text))
            }
            Ty::List(of) => self.list(of, at),
            Ty::Optional(of) => {
                self.take(OPTIONAL_BYTES)?;
                match self.u32_at(at)? {
                    0 => Ok(Value::Nothing),
                    present => self.slot(of, present),
                }
            }
            Ty::Record(index) => {
                self.take(HEADER_BYTES)?;
                let layout = self.types.record(*index).map_err(|_| Corrupt)?;
                let name = self.types.name(*index).ok_or(Corrupt)?;
                // The whole slot is in memory before any field is read.
                self.bytes(at, layout.slot)?;
                let mut fields = Vec::with_capacity(layout.fields.len());
                let mut optional = Vec::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    let value = self.slot(&field.ty, at.checked_add(field.offset).ok_or(Corrupt)?)?;
                    fields.push((field.name.clone(), value));
                    optional.push(matches!(field.ty, Ty::Optional(_)));
                }
                let value = Value::record_of(name, fields, &optional);
                // The size emitted code charges a value that holds the record (D-119).
                (self.u64_at(at)? == value_bytes(&value))
                    .then_some(value)
                    .ok_or(Corrupt)
            }
        }
    }

    /// The two `i32` of a text or a list in the slot at `at`.
    fn pair(&self, at: u32) -> Result<(u32, u32), Corrupt> {
        Ok((self.u32_at(at)?, self.u32_at(at.checked_add(4).ok_or(Corrupt)?)?))
    }

    fn list(&mut self, of: &Ty, at: u32) -> Result<Value, Corrupt> {
        let (ptr, len) = self.pair(at)?;
        if u64::from(len) > MAX_LIST_SIZE {
            return Err(Corrupt);
        }
        self.take(HEADER_BYTES)?;
        let prefix = self.u64_at(ptr.checked_sub(LIST_PREFIX).ok_or(Corrupt)?)?;
        let slot = self.types.slot(of).map_err(|_| Corrupt)?;
        let optional = matches!(of, Ty::Optional(_));
        let value = if slot == 0 {
            if let Some(shared) = self.nothings.get(&len) {
                shared.clone()
            } else {
                // Made once for each length, and paid for before it is made.
                self.items = self.items.checked_sub(u64::from(len)).ok_or(Corrupt)?;
                let made = Value::list_of(vec![Value::Nothing; len as usize], optional);
                self.nothings.insert(len, made.clone());
                made
            }
        } else {
            // Every item's slot is in memory before any is read.
            self.bytes(ptr, slot.checked_mul(len).ok_or(Corrupt)?)?;
            let mut items = Vec::with_capacity(len as usize);
            for i in 0..len {
                items.push(self.slot(of, ptr.checked_add(i * slot).ok_or(Corrupt)?)?);
            }
            Value::list_of(items, optional)
        };
        // The size emitted code charges a value that holds the list (D-119).
        (prefix == value_bytes(&value)).then_some(value).ok_or(Corrupt)
    }
}
