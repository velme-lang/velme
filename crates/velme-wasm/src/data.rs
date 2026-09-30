//! Literal values as a data segment (`runtime/31` R-SBX-19 (c)): a `literal` node is decoded once, by the validator
//! (D-83), and is laid out here in the layout of §3, so evaluating it copies nothing and allocates nothing.

use velme_builtins::Value;

use crate::EmitError;
use crate::abi::{LIST_PREFIX, RECORD_PREFIX, SCRATCH_BYTES};
use crate::ty::{Ty, Types};

/// A constant on the stack.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Const {
    I32(i32),
    I64(i64),
}

/// The bytes of the data segment, which starts at [`SCRATCH_BYTES`].
#[derive(Debug, Default)]
pub(crate) struct Data {
    pub(crate) bytes: Vec<u8>,
}

fn mismatch() -> EmitError {
    EmitError::Internal("a literal's value doesn't match its type".to_owned())
}

impl Data {
    /// Places `bytes` at the next 8-aligned address, which it returns.
    fn place(&mut self, bytes: &[u8]) -> Result<u32, EmitError> {
        let at = self.bytes.len().next_multiple_of(8);
        self.bytes.resize(at, 0);
        self.bytes.extend_from_slice(bytes);
        let address = u32::try_from(at).ok().and_then(|at| at.checked_add(SCRATCH_BYTES));
        // An address is an `i32` in emitted code.
        address
            .filter(|address| i32::try_from(*address).is_ok())
            .ok_or(EmitError::Declined("literals too large for one data segment"))
    }

    /// `value` of type `ty` as the constants of its flat form ([`Types::flat`]), placing what it points at.
    pub(crate) fn flat(&mut self, value: &Value, ty: &Ty, types: &Types) -> Result<Vec<Const>, EmitError> {
        Ok(match (ty, value) {
            (Ty::Number, Value::Number(x)) => {
                let (lo, hi) = x.to_bits();
                vec![Const::I64(lo.cast_signed()), Const::I64(hi.cast_signed())]
            }
            (Ty::Boolean, Value::Boolean(b)) => vec![Const::I32(i32::from(*b))],
            (Ty::Nothing, Value::Nothing) => Vec::new(),
            (Ty::Text, Value::Text(text)) => {
                let at = self.place(text.as_bytes())?;
                vec![address(at), length(text.len())?]
            }
            (Ty::List(of), Value::List(list)) => {
                let mut image = list.bytes().to_le_bytes().to_vec();
                for item in list.iter() {
                    image.extend(self.slot(item, of, types)?);
                }
                let at = self.place(&image)?;
                vec![address(at.saturating_add(LIST_PREFIX)), length(list.len())?]
            }
            (Ty::Optional(of), Value::Nothing) => {
                let mut flat = vec![Const::I32(0)];
                flat.extend(types.flat(of).into_iter().map(|v| match v {
                    crate::code::V::I32 => Const::I32(0),
                    crate::code::V::I64 => Const::I64(0),
                }));
                flat
            }
            (Ty::Optional(of), present) => {
                let mut flat = vec![Const::I32(1)];
                flat.extend(self.flat(present, of, types)?);
                flat
            }
            (Ty::Record(_), Value::Record(_)) => vec![address(self.boxed(value, ty, types)?)],
            _ => return Err(mismatch()),
        })
    }

    /// Places the slot of `value` and returns its address.
    fn boxed(&mut self, value: &Value, ty: &Ty, types: &Types) -> Result<u32, EmitError> {
        let image = self.slot(value, ty, types)?;
        self.place(&image)
    }

    /// `value` of type `ty` as the bytes of its slot (§3), placing what it points at.
    fn slot(&mut self, value: &Value, ty: &Ty, types: &Types) -> Result<Vec<u8>, EmitError> {
        let mut image = Vec::new();
        match (ty, value) {
            // A `T?` is a pointer to its `T`, or 0.
            (Ty::Optional(_), Value::Nothing) => image.extend(0u64.to_le_bytes()),
            (Ty::Optional(of), present) => {
                let at = self.boxed(present, of, types)?;
                image.extend(u64::from(at).to_le_bytes());
            }
            (Ty::Record(index), Value::Record(record)) => {
                let fields = &types.record(*index)?.fields;
                if fields.len() != record.fields.len() {
                    return Err(mismatch());
                }
                image.extend(record.bytes().to_le_bytes());
                debug_assert_eq!(image.len(), RECORD_PREFIX as usize);
                for (field, (_, value)) in fields.iter().zip(&record.fields) {
                    image.extend(self.slot(value, &field.ty, types)?);
                }
            }
            _ => {
                for constant in self.flat(value, ty, types)? {
                    match constant {
                        Const::I32(x) => image.extend(x.to_le_bytes()),
                        Const::I64(x) => image.extend(x.to_le_bytes()),
                    }
                }
                // A `Boolean` is one `i32` in a slot of 8.
                image.resize(types.slot(ty)? as usize, 0);
            }
        }
        Ok(image)
    }
}

fn address(at: u32) -> Const {
    Const::I32(at.cast_signed())
}

fn length(len: usize) -> Result<Const, EmitError> {
    i32::try_from(len).map(Const::I32).map_err(|_| mismatch())
}
