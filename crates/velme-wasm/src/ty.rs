//! The static types of a goal's IR and their layout in linear memory (`runtime/31` §3). The interpreter's values
//! carry their shape; emitted code has only the types, so every node is typed here first, by the validator's rules
//! (`compiler/21` §6, stage 4) over IR the validator has already accepted.

use std::collections::HashMap;

use velme_builtins::memory::{BOOLEAN_BYTES, HEADER_BYTES, NUMBER_BYTES};
use velme_builtins::{Builtin, Function};
use velme_ir::{BinaryOperator, Goal, Node, Type, UnaryOperator};

use crate::EmitError;
use crate::abi::RECORD_PREFIX;
use crate::code::V;

/// A value type with its record names resolved. An optional is never of an optional (`compiler/21` §2.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Ty {
    Number,
    Boolean,
    /// The type of a bare `nothing`; it takes no slot, and the only type that holds it is a list.
    Nothing,
    Text,
    List(Box<Ty>),
    Optional(Box<Ty>),
    /// A record, by its index in [`Types`].
    Record(usize),
}

impl Ty {
    /// `T?`, where `T??` is `T?`.
    pub(crate) fn optional(self) -> Ty {
        match self {
            Ty::Optional(_) => self,
            other => Ty::Optional(Box::new(other)),
        }
    }
}

/// R-TYP-20: `from` is assignable to `to`.
pub(crate) fn assignable(from: &Ty, to: &Ty) -> bool {
    from == to || matches!(to, Ty::Optional(inner) if *from == Ty::Nothing || from == &**inner)
}

/// R-TYP-26: the least type both are assignable to.
pub(crate) fn join(a: &Ty, b: &Ty) -> Option<Ty> {
    if assignable(a, b) {
        Some(b.clone())
    } else if assignable(b, a) {
        Some(a.clone())
    } else if *a == Ty::Nothing {
        Some(b.clone().optional())
    } else if *b == Ty::Nothing {
        Some(a.clone().optional())
    } else {
        None
    }
}

/// A record type laid out: its logical size, then its fields in declared order, each in its slot (§3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) fields: Vec<Field>,
    /// The bytes of its slot, the size before the fields included.
    pub(crate) slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// Where its slot starts in the record's.
    pub(crate) offset: u32,
}

/// The record types of a goal. One the emitter has no layout for says why, and declines the goal only if the goal
/// uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Types {
    names: Vec<String>,
    records: Vec<Result<Record, &'static str>>,
}

impl Types {
    pub(crate) fn new(goal: &Goal) -> Result<Types, EmitError> {
        let mut types = Types {
            names: goal.types.keys().cloned().collect(),
            records: Vec::new(),
        };
        let mut fields = Vec::with_capacity(types.names.len());
        for record in goal.types.values() {
            let resolved: Result<Vec<(String, Ty)>, EmitError> = record
                .fields
                .iter()
                .map(|(name, ty)| Ok((name.clone(), inside(types.resolve(ty)?)?)))
                .collect();
            fields.push(resolved?);
        }
        // A record's slot holds its fields' slots, so a record is laid out after the records it holds.
        let mut slots: Vec<Option<Result<u32, &'static str>>> = vec![None; fields.len()];
        let mut open = vec![false; fields.len()];
        let known: Vec<Result<u32, &'static str>> = (0..fields.len())
            .map(|index| record_slot(index, &fields, &mut slots, &mut open))
            .collect();
        for (fields, slot) in fields.into_iter().zip(known) {
            let laid = slot.and_then(|slot| {
                let mut offset = RECORD_PREFIX;
                let mut laid = Vec::with_capacity(fields.len());
                for (name, ty) in fields {
                    let size = slot_with(&ty, &slots)?;
                    laid.push(Field { name, ty, offset });
                    offset = offset.saturating_add(size);
                }
                Ok(Record { fields: laid, slot })
            });
            types.records.push(laid);
        }
        Ok(types)
    }

    /// `ty` with its record names resolved.
    pub(crate) fn resolve(&self, ty: &Type) -> Result<Ty, EmitError> {
        Ok(match ty {
            Type::Number {} => Ty::Number,
            Type::Text {} => Ty::Text,
            Type::Boolean {} => Ty::Boolean,
            Type::Nothing {} => Ty::Nothing,
            Type::Optional { of } => inside(self.resolve(of)?)?.optional(),
            // A list of `Nothing` is a length: its items take no slot and no bytes (`runtime/30` §7.1).
            Type::List { of } => Ty::List(Box::new(self.resolve(of)?)),
            Type::Record { name } => Ty::Record(self.names.iter().position(|n| n == name).ok_or_else(internal)?),
        })
    }

    /// The layout of record `index`, which the goal uses: one without a layout declines the goal (R-SBX-02).
    pub(crate) fn record(&self, index: usize) -> Result<&Record, EmitError> {
        match self.records.get(index) {
            Some(Ok(record)) => Ok(record),
            Some(Err(why)) => Err(EmitError::Declined(why)),
            None => Err(internal()),
        }
    }

    /// The name of record `index`.
    pub(crate) fn name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }

    /// The bytes of a slot of `ty` (§3).
    pub(crate) fn slot(&self, ty: &Ty) -> Result<u32, EmitError> {
        Ok(match ty {
            Ty::Record(index) => self.record(*index)?.slot,
            scalar => scalar_slot(scalar),
        })
    }

    /// `ty` as values on the stack or in locals: a `Number` is `lo` and `hi`, a text or list its pointer and length,
    /// an optional its tag and then its value, and a record the address of its slot. Only in memory is an optional
    /// boxed (§3).
    pub(crate) fn flat(&self, ty: &Ty) -> Vec<V> {
        match ty {
            Ty::Number => vec![V::I64, V::I64],
            Ty::Boolean | Ty::Record(_) => vec![V::I32],
            Ty::Nothing => Vec::new(),
            Ty::Text | Ty::List(_) => vec![V::I32, V::I32],
            Ty::Optional(of) => {
                let mut flat = vec![V::I32];
                flat.extend(self.flat(of));
                flat
            }
        }
    }

    /// The logical size of any value of `ty` (`runtime/30` §7.1), if it is the same for all of them.
    pub(crate) fn fixed_bytes(&self, ty: &Ty) -> Option<u64> {
        match ty {
            Ty::Number => Some(NUMBER_BYTES),
            Ty::Boolean => Some(BOOLEAN_BYTES),
            Ty::Nothing => Some(0),
            Ty::Text | Ty::List(_) | Ty::Optional(_) => None,
            Ty::Record(index) => {
                let record = self.records.get(*index)?.as_ref().ok()?;
                record.fields.iter().try_fold(HEADER_BYTES, |total, field| {
                    Some(total.saturating_add(self.fixed_bytes(&field.ty)?))
                })
            }
        }
    }
}

fn internal() -> EmitError {
    EmitError::Internal("the IR names a type the validator should have rejected".to_owned())
}

/// What the host needs of a goal to call its module (R-SBX-03): the layouts the module was emitted with, so the two
/// sides cannot disagree on one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) types: Types,
    /// The inputs in declared order, each with where its slot starts from `input_ptr`.
    pub(crate) inputs: Vec<(Ty, u32)>,
    /// The bytes of the inputs' slots together.
    pub(crate) input_bytes: u32,
    pub(crate) output: Ty,
}

/// The slot of a type that isn't a record (§3). A `T?` is one pointer, whatever `T` is, so an absent one takes no
/// more than the 8 bytes it is charged (`runtime/30` §7.1, D-119).
fn scalar_slot(ty: &Ty) -> u32 {
    match ty {
        Ty::Number => 16,
        Ty::Boolean | Ty::Text | Ty::List(_) | Ty::Optional(_) => 8,
        Ty::Nothing | Ty::Record(_) => 0,
    }
}

/// The slot of `ty`, given the record slots worked out so far.
fn slot_with(ty: &Ty, slots: &[Option<Result<u32, &'static str>>]) -> Result<u32, &'static str> {
    match ty {
        Ty::Record(index) => slots.get(*index).copied().flatten().unwrap_or(Err(HOLDS_ITSELF)),
        scalar => Ok(scalar_slot(scalar)),
    }
}

/// Works out the slot of record `index` and of the records it holds inline. A list or an optional is a pointer, so
/// a record may reach itself through one; one that holds itself inline has no layout.
fn record_slot(
    index: usize,
    fields: &[Vec<(String, Ty)>],
    slots: &mut Vec<Option<Result<u32, &'static str>>>,
    open: &mut Vec<bool>,
) -> Result<u32, &'static str> {
    if let Some(known) = slots.get(index).copied().flatten() {
        return known;
    }
    if open.get(index).copied().unwrap_or(true) {
        return Err(HOLDS_ITSELF);
    }
    if let Some(flag) = open.get_mut(index) {
        *flag = true;
    }
    let mut total = Ok(RECORD_PREFIX);
    for (_, ty) in fields.get(index).map(Vec::as_slice).unwrap_or_default() {
        let size = match ty {
            Ty::Record(held) => record_slot(*held, fields, slots, open),
            scalar => Ok(scalar_slot(scalar)),
        };
        // An address is an `i32` in emitted code.
        total = total.and_then(|total: u32| {
            total
                .checked_add(size?)
                .filter(|total| i32::try_from(*total).is_ok())
                .ok_or(TOO_WIDE)
        });
    }
    if let Some(slot) = slots.get_mut(index) {
        *slot = Some(total);
    }
    total
}

const HOLDS_ITSELF: &str = "a record type that holds itself";

/// A record whose slot no address reaches. It is about addresses only: no scratch frame holds a record (R-SBX-19).
const TOO_WIDE: &str = "a record type too wide for one memory";

/// The type of every node of a goal's body, by the node's address: looked up, never iterated (CC-DET-01).
#[derive(Debug)]
pub(crate) struct Typing {
    of: HashMap<*const Node, Ty>,
}

impl Typing {
    /// Types `body` with `inputs` in scope.
    pub(crate) fn new(types: &Types, inputs: &[(&str, Ty)], body: &Node) -> Result<Typing, EmitError> {
        let mut typer = Typer {
            types,
            inputs,
            scope: Vec::new(),
            of: HashMap::new(),
        };
        typer.node(body)?;
        Ok(Typing { of: typer.of })
    }

    pub(crate) fn of(&self, node: &Node) -> Result<&Ty, EmitError> {
        self.of.get(&std::ptr::from_ref(node)).ok_or_else(untyped)
    }
}

fn untyped() -> EmitError {
    EmitError::Internal("the IR has a node the validator should have rejected".to_owned())
}

struct Typer<'a> {
    types: &'a Types,
    inputs: &'a [(&'a str, Ty)],
    scope: Vec<(&'a str, Ty)>,
    of: HashMap<*const Node, Ty>,
}

impl<'a> Typer<'a> {
    fn node(&mut self, node: &'a Node) -> Result<Ty, EmitError> {
        let ty = self.kind(node)?;
        self.of.insert(std::ptr::from_ref(node), ty.clone());
        Ok(ty)
    }

    fn kind(&mut self, node: &'a Node) -> Result<Ty, EmitError> {
        Ok(match node {
            Node::Literal { ty, .. } => self.types.resolve(ty)?,
            Node::Input { name } => lookup(self.inputs, name)?,
            Node::Local { name } => lookup(&self.scope, name)?,
            Node::Record { ty, fields } => {
                for field in fields.values() {
                    self.node(field)?;
                }
                self.types.resolve(&Type::Record { name: ty.clone() })?
            }
            Node::List { of, items } => {
                for item in items {
                    self.node(item)?;
                }
                Ty::List(Box::new(self.types.resolve(of)?))
            }
            Node::FieldGet { of, field } => {
                let Ty::Record(index) = self.node(of)? else {
                    return Err(untyped());
                };
                let record = self.types.record(index)?;
                let found = record.fields.iter().find(|f| f.name == *field).ok_or_else(untyped)?;
                found.ty.clone()
            }
            Node::BinaryOp { op, left, right } => {
                self.node(left)?;
                self.node(right)?;
                match op {
                    BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul | BinaryOperator::Div => Ty::Number,
                    BinaryOperator::Lt
                    | BinaryOperator::Le
                    | BinaryOperator::Gt
                    | BinaryOperator::Ge
                    | BinaryOperator::Eq
                    | BinaryOperator::Ne
                    | BinaryOperator::And
                    | BinaryOperator::Or => Ty::Boolean,
                }
            }
            Node::UnaryOp { op, arg } => {
                self.node(arg)?;
                match op {
                    UnaryOperator::Neg => Ty::Number,
                    UnaryOperator::Not | UnaryOperator::IsEmpty => Ty::Boolean,
                }
            }
            Node::Let { bind, body } => {
                let mark = self.scope.len();
                for (name, value) in bind {
                    let ty = self.node(value)?;
                    self.scope.push((name, ty));
                }
                let ty = self.node(body)?;
                self.scope.truncate(mark);
                ty
            }
            Node::Condition { cond, then, otherwise } => {
                self.node(cond)?;
                let (a, b) = (self.node(then)?, self.node(otherwise)?);
                join(&a, &b).ok_or_else(untyped)?
            }
            Node::Narrow { of, default } => {
                let found = self.node(of)?;
                let fallback = self.node(default)?;
                match found {
                    Ty::Optional(inner) => *inner,
                    Ty::Nothing => fallback,
                    _ => return Err(untyped()),
                }
            }
            Node::Map { list, func, items } => {
                let element = self.element(list)?;
                let result = self.lambda(&func.param, element, &func.body)?;
                // The validator typed the body too (§7.1): the two must agree on what the items cost.
                if items.optional() != Some(matches!(result, Ty::Optional(_))) {
                    return Err(untyped());
                }
                Ty::List(Box::new(result))
            }
            Node::Filter { list, func } | Node::Sort { list, key: func, .. } => {
                let element = self.element(list)?;
                self.lambda(&func.param, element.clone(), &func.body)?;
                Ty::List(Box::new(element))
            }
            Node::Find { list, func } => {
                let element = self.element(list)?;
                self.lambda(&func.param, element.clone(), &func.body)?;
                inside(element)?.optional()
            }
            Node::All { list, func } | Node::Any { list, func } => {
                let element = self.element(list)?;
                self.lambda(&func.param, element, &func.body)?;
                Ty::Boolean
            }
            Node::Reduce { list, init, func } => {
                let element = self.element(list)?;
                let acc = self.node(init)?;
                let mark = self.scope.len();
                self.scope.push((&func.acc, acc.clone()));
                self.scope.push((&func.param, element));
                self.node(&func.body)?;
                self.scope.truncate(mark);
                acc
            }
            Node::Builtin { name, args } => {
                for arg in args {
                    self.node(arg)?;
                }
                let function = Builtin::find(name).and_then(|b| b.function).ok_or_else(untyped)?;
                match function {
                    Function::Length
                    | Function::Sum
                    | Function::Abs
                    | Function::Floor
                    | Function::Ceil
                    | Function::Round
                    | Function::Clamp
                    | Function::Random => Ty::Number,
                    Function::IsEmpty | Function::Contains => Ty::Boolean,
                    Function::Maximum | Function::Minimum => Ty::Number.optional(),
                    Function::Concat | Function::ToText => Ty::Text,
                    Function::Range => Ty::List(Box::new(Ty::Number)),
                }
            }
            Node::Call(_) => return Err(untyped()),
        })
    }

    /// The element type of the list `node` gives.
    fn element(&mut self, node: &'a Node) -> Result<Ty, EmitError> {
        match self.node(node)? {
            Ty::List(element) => Ok(*element),
            _ => Err(untyped()),
        }
    }

    /// The type of `body` with `param` bound to `element`.
    fn lambda(&mut self, param: &'a str, element: Ty, body: &'a Node) -> Result<Ty, EmitError> {
        let mark = self.scope.len();
        self.scope.push((param, element));
        let ty = self.node(body);
        self.scope.truncate(mark);
        ty
    }
}

/// `ty` as the type of a record field or of a present optional.
fn inside(ty: Ty) -> Result<Ty, EmitError> {
    match ty {
        Ty::Nothing => Err(EmitError::Declined("a type that holds Nothing")),
        ty => Ok(ty),
    }
}

fn lookup(scope: &[(&str, Ty)], name: &str) -> Result<Ty, EmitError> {
    scope
        .iter()
        .rev()
        .find(|(n, _)| *n == name)
        .map(|(_, ty)| ty.clone())
        .ok_or_else(untyped)
}
