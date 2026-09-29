//! The JSON value mapping of `language/11` §10 (D-23), shared by `velme run` input and output, examples, fixtures and
//! IR literals: JSON decoded against a type into a [`Value`], and a value encoded back.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value as Json;
use velme_builtins::limits::MAX_LIST_SIZE;
use velme_builtins::{Number, Value};
use velme_diagnostics::{Code, did_you_mean};
use velme_sema::hir::{Program, RecordType, Type};

use crate::json::{pointer, write_string};
use crate::node::{RecordType as IrRecordType, Type as IrType};
use crate::{MAX_JSON_DEPTH, ParseError, from_json_str};

/// Why JSON doesn't decode as a type: the first problem, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// Where, as path segments from the decoded value (array indexes and object keys).
    pub path: Vec<String>,
    /// What is wrong there.
    pub problem: DecodeProblem,
}

impl DecodeError {
    /// The diagnostic code: `VL0902 InvalidInput`, or `VL0606` for a list above `max_list_size` (R-TYP-24).
    pub fn code(&self) -> Code {
        match self.problem {
            DecodeProblem::TooManyItems { .. } => Code::SizeLimitExceeded,
            _ => Code::InvalidInput,
        }
    }

    /// [`DecodeError::path`] as a JSON Pointer (RFC 6901).
    pub fn pointer(&self) -> String {
        pointer(&self.path)
    }
}

/// One way JSON fails the mapping (`language/11` §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeProblem {
    /// The text isn't JSON, repeats an object key (`compiler/21` R-IR-21) or nests too deep; the path is where, when
    /// known.
    InvalidJson {
        /// What is wrong, for a note.
        reason: String,
    },
    /// The JSON value has the wrong shape for the type, e.g. `null` for a non-nullable type.
    Mismatch {
        /// The expected type as a learner writes it.
        expected: String,
        /// What the JSON holds instead, as `reference/90`'s VL0902 message words it: `text`, `a number`, `nothing`,
        /// `a list`, `a record`, `true` or `false`.
        found: &'static str,
    },
    /// A number no `Number` holds exactly (R-TYP-04).
    NumberOutOfRange {
        /// The number as written.
        number: String,
    },
    /// An object key the record doesn't declare; the path ends at that key.
    UnknownField {
        /// The record type.
        record: String,
        /// The key.
        field: String,
        /// A declared field it may be a misspelling of.
        help: Option<String>,
    },
    /// A declared field the object lacks.
    MissingField {
        /// The record type.
        record: String,
        /// The field.
        field: String,
    },
    /// An array longer than `max_list_size` (R-TYP-24).
    TooManyItems {
        /// Its length.
        items: usize,
    },
}

impl fmt::Display for DecodeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeProblem::InvalidJson { reason } => write!(f, "this isn't valid JSON: {reason}"),
            DecodeProblem::Mismatch { expected, .. } => write!(f, "this value isn't {expected}"),
            DecodeProblem::NumberOutOfRange { number } => {
                write!(f, "the number {number} is outside the range of Number")
            }
            DecodeProblem::UnknownField { record, field, .. } => write!(f, "`{record}` has no field `{field}`"),
            DecodeProblem::MissingField { record, field } => write!(f, "a `{record}` needs the field `{field}`"),
            DecodeProblem::TooManyItems { items } => {
                write!(f, "this list has {items} items; at most {MAX_LIST_SIZE} are allowed")
            }
        }
    }
}

/// JSON `text` decoded as a value of `ty`, whose records are declared in `program`: the entry point for input JSON
/// (`velme run` inputs, examples, fixtures). The text is read by [`crate::from_json_str`], so an object that repeats a
/// key is rejected (D-21), then decoded by [`decode_value`].
pub fn decode_str(text: &str, ty: &Type, program: &Program) -> Result<Value, DecodeError> {
    let json: Json = from_json_str(text).map_err(|error| {
        let (at, reason) = match error {
            ParseError::DuplicateKey { pointer } => (pointer, "an object repeats this key".to_owned()),
            ParseError::ReservedKey { pointer } => (pointer, "this object key is reserved".to_owned()),
            ParseError::TooDeep { pointer } => (pointer, format!("it nests deeper than {MAX_JSON_DEPTH} levels")),
            ParseError::Json(error) => (String::new(), error.to_string()),
        };
        DecodeError {
            path: segments(&at),
            problem: DecodeProblem::InvalidJson { reason },
        }
    })?;
    decode_value(&json, ty, program)
}

/// The segments of a JSON Pointer (RFC 6901).
fn segments(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// `json`, already parsed, decoded as a value of `ty`, whose records are declared in `program` (`language/11` §10,
/// D-23). Numbers are read exactly from their text (serde_json's `arbitrary_precision`, CC-DET-03); `-0` is `0`. A
/// type that already has an error (`compiler/20` R-CMP-10) accepts any JSON, as `nothing`, so the validator reports
/// nothing more about it; a program from `analyze` has no such type.
pub fn decode_value(json: &Json, ty: &Type, program: &Program) -> Result<Value, DecodeError> {
    let mut path = Vec::new();
    decode_at(json, ty, program, &mut path).map_err(|problem| DecodeError { path, problem })
}

fn decode_at(json: &Json, ty: &Type, program: &Program, path: &mut Vec<String>) -> Result<Value, DecodeProblem> {
    let mismatch = || DecodeProblem::Mismatch {
        expected: program.type_name(ty),
        found: json_kind(json),
    };
    match (ty, json) {
        (Type::Error, _) => Ok(Value::Nothing),
        (Type::Number, Json::Number(n)) => {
            let text = n.to_string();
            Number::parse(&text)
                .map(Value::Number)
                .ok_or(DecodeProblem::NumberOutOfRange { number: text })
        }
        (Type::Text, Json::String(s)) => Ok(Value::text(s)),
        (Type::Boolean, Json::Bool(b)) => Ok(Value::Boolean(*b)),
        (Type::Nothing | Type::Optional(_), Json::Null) => Ok(Value::Nothing),
        (Type::Optional(inner), _) => decode_at(json, inner, program, path),
        (Type::List(element), Json::Array(items)) => {
            let mut values = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                path.push(i.to_string());
                values.push(decode_at(item, element, program, path)?);
                path.pop();
            }
            // After the items, so a wrong item is reported even in a list that is also too long.
            if u64::try_from(items.len()).map_or(true, |n| n > MAX_LIST_SIZE) {
                return Err(DecodeProblem::TooManyItems { items: items.len() });
            }
            Ok(Value::list(values))
        }
        (Type::Record(id), Json::Object(members)) => match program.record(*id) {
            Some(record) => decode_record(record, members, program, path),
            None => Err(mismatch()),
        },
        _ => Err(mismatch()),
    }
}

/// What a JSON value holds, in the words of `reference/90`'s VL0902 message.
pub fn json_kind(json: &Json) -> &'static str {
    match json {
        Json::Null => "nothing",
        Json::Bool(true) => "true",
        Json::Bool(false) => "false",
        Json::Number(_) => "a number",
        Json::String(_) => "text",
        Json::Array(_) => "a list",
        Json::Object(_) => "a record",
    }
}

/// A record value has exactly the declared fields, each of its type; fields come out in declaration order.
fn decode_record(
    record: &RecordType,
    members: &serde_json::Map<String, Json>,
    program: &Program,
    path: &mut Vec<String>,
) -> Result<Value, DecodeProblem> {
    if let Some(extra) = members.keys().find(|k| record.field(k).is_none()) {
        path.push(extra.clone());
        return Err(DecodeProblem::UnknownField {
            record: record.name.clone(),
            field: extra.clone(),
            help: did_you_mean(extra, record.fields.iter().map(|f| f.name.as_str())),
        });
    }
    let mut fields = Vec::with_capacity(record.fields.len());
    for field in &record.fields {
        let Some(member) = members.get(&field.name) else {
            return Err(DecodeProblem::MissingField {
                record: record.name.clone(),
                field: field.name.clone(),
            });
        };
        path.push(field.name.clone());
        fields.push((field.name.clone(), decode_at(member, &field.ty, program, path)?));
        path.pop();
    }
    Ok(Value::record(&record.name, fields))
}

/// A `literal` node's value, for a back end, which has the IR's own `types` but no HIR: the same mapping as
/// [`decode_value`], which the validator already decoded it with (`compiler/21` §6 stage 4). `None` only for IR that
/// didn't pass the validator.
pub fn decode_literal(json: &Json, ty: &IrType, types: &BTreeMap<String, IrRecordType>) -> Option<Value> {
    match (ty, json) {
        (IrType::Number {}, Json::Number(n)) => Number::parse(&n.to_string()).map(Value::Number),
        (IrType::Text {}, Json::String(s)) => Some(Value::text(s)),
        (IrType::Boolean {}, Json::Bool(b)) => Some(Value::Boolean(*b)),
        (IrType::Nothing {} | IrType::Optional { .. }, Json::Null) => Some(Value::Nothing),
        (IrType::Optional { of }, _) => decode_literal(json, of, types),
        (IrType::List { of }, Json::Array(items)) => items
            .iter()
            .map(|item| decode_literal(item, of, types))
            .collect::<Option<Vec<_>>>()
            .map(Value::list),
        (IrType::Record { name }, Json::Object(members)) => {
            let record = types.get(name).filter(|r| r.fields.len() == members.len())?;
            let fields = record
                .fields
                .iter()
                .map(|(field, ty)| Some((field.clone(), decode_literal(members.get(field)?, ty, types)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Value::record(name, fields))
        }
        _ => None,
    }
}

/// `value` as JSON output (`language/11` R-TYP-23): compact, records' fields in declaration order, numbers per
/// R-TYP-08. Hashing uses the separate sorted-key form of D-21 ([`crate::to_canonical_string`]).
pub fn encode_value(value: &Value) -> String {
    let mut out = String::new();
    encode_into(value, &mut out);
    out
}

/// Items of a list shown in the human view before the rest are counted (`runtime/30` R-RUN-20).
pub const SHOWN_ITEMS: usize = 10;

/// Characters of a text shown in the human view before the rest are counted (R-RUN-20).
pub const SHOWN_CHARS: usize = 80;

/// `value` for the human view (`runtime/30` R-RUN-20): the JSON of [`encode_value`], with a list past
/// [`SHOWN_ITEMS`] items and a text past [`SHOWN_CHARS`] characters cut, and a count of what was left out. Check reports
/// and traces both render values this way; `--json` output keeps them whole.
pub fn display_value(value: &Value) -> String {
    let mut out = String::new();
    display_into(value, &mut out);
    out
}

fn display_into(value: &Value, out: &mut String) {
    match value {
        Value::Text(t) if t.chars().count() > SHOWN_CHARS => {
            let shown: String = t.chars().take(SHOWN_CHARS).collect();
            write_string(&shown, out);
            let more = t.chars().count() - SHOWN_CHARS;
            out.push_str(&format!("…(+{more} characters)"));
        }
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().take(SHOWN_ITEMS).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                display_into(item, out);
            }
            if items.len() > SHOWN_ITEMS {
                out.push_str(&format!(",…(+{} items)", items.len() - SHOWN_ITEMS));
            }
            out.push(']');
        }
        Value::Record(record) => {
            out.push('{');
            for (i, (name, item)) in record.fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                display_into(item, out);
            }
            out.push('}');
        }
        _ => encode_into(value, out),
    }
}

fn encode_into(value: &Value, out: &mut String) {
    match value {
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::Text(t) => write_string(t, out),
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Nothing => out.push_str("null"),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_into(item, out);
            }
            out.push(']');
        }
        Value::Record(record) => {
            out.push('{');
            for (i, (name, item)) in record.fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                encode_into(item, out);
            }
            out.push('}');
        }
    }
}
