//! Canonical JSON (D-21, `compiler/21` R-IR-21) and strict JSON input (duplicate keys rejected).

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use serde::Serialize;
use serde::de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use velme_builtins::Number;

/// Why a value has no canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// A number no Velme `Number` holds exactly (R-TYP-04, D-23), so it has no R-TYP-08 rendering.
    NumberOutOfRange {
        /// JSON Pointer to the number.
        pointer: String,
        /// The number as written.
        number: String,
    },
    /// The Rust value did not serialize to JSON (e.g. a map with non-string keys).
    Serialize(String),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NumberOutOfRange { pointer, number } => {
                write!(f, "number {number} at `{pointer}` is outside the range of Number")
            }
            Self::Serialize(message) => write!(f, "value is not JSON: {message}"),
        }
    }
}

impl std::error::Error for CanonicalError {}

/// Why JSON input was rejected.
#[derive(Debug)]
pub enum ParseError {
    /// An object repeats a key (R-IR-21).
    DuplicateKey {
        /// JSON Pointer to the repeated member.
        pointer: String,
    },
    /// An object key that serde_json reserves for exact numbers, which would otherwise decode as a number.
    ReservedKey {
        /// JSON Pointer to the member.
        pointer: String,
    },
    /// Arrays and objects nest deeper than [`MAX_JSON_DEPTH`].
    TooDeep {
        /// JSON Pointer to the first value past the limit.
        pointer: String,
    },
    /// Malformed JSON, or JSON that does not have the expected shape.
    Json(serde_json::Error),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateKey { pointer } => write!(f, "duplicate object key at `{pointer}`"),
            Self::ReservedKey { pointer } => write!(f, "reserved object key at `{pointer}`"),
            Self::TooDeep { pointer } => {
                write!(f, "JSON nests deeper than {MAX_JSON_DEPTH} levels at `{pointer}`")
            }
            Self::Json(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {}

/// `value` as canonical JSON (R-IR-21): the bytes every hash and artifact uses.
pub fn to_canonical_string<T: Serialize + ?Sized>(value: &T) -> Result<String, CanonicalError> {
    let value = serde_json::to_value(value).map_err(|e| CanonicalError::Serialize(e.to_string()))?;
    let mut out = String::new();
    write_value(&value, &mut out, &mut Vec::new())?;
    Ok(out)
}

/// How deep arrays and objects may nest in [`from_json_str`] input. IR at the §7 expression depth reaches about 390
/// levels (three per `let`, plus the envelope and a literal), which stage 7 checks with `VL0402`; this guard sits above
/// that and bounds the parser's recursion, which must fit a 2 MiB thread stack even in debug builds (R-IR-18).
pub const MAX_JSON_DEPTH: usize = 512;

/// Parses `text` as `T`, rejecting any object that repeats a key (R-IR-21), uses a key serde_json reserves, or nests
/// deeper than [`MAX_JSON_DEPTH`]. JSON input is read through this function only.
pub fn from_json_str<T: DeserializeOwned>(text: &str) -> Result<T, ParseError> {
    let mut check = Walk {
        path: Vec::new(),
        failure: None,
    };
    let mut de = serde_json::Deserializer::from_str(text);
    // serde_json's own limit (128) counts JSON levels, not IR nesting; `Walk` enforces `MAX_JSON_DEPTH` instead.
    de.disable_recursion_limit();
    if let Err(error) = (&mut check).deserialize(&mut de).and_then(|()| de.end()) {
        return Err(check.failure.unwrap_or(ParseError::Json(error)));
    }
    let mut de = serde_json::Deserializer::from_str(text);
    de.disable_recursion_limit();
    T::deserialize(&mut de)
        .and_then(|value| de.end().map(|()| value))
        .map_err(ParseError::Json)
}

fn write_value(value: &Value, out: &mut String, path: &mut Vec<String>) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            let text = n.to_string();
            let Some(number) = canonical_number(&text) else {
                return Err(CanonicalError::NumberOutOfRange {
                    pointer: pointer(path),
                    number: text,
                });
            };
            out.push_str(&number);
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                path.push(i.to_string());
                write_value(item, out, path)?;
                path.pop();
            }
            out.push(']');
        }
        Value::Object(map) => {
            // `String` order is UTF-8 byte order, which is code point order (R-IR-21); sorted here rather than
            // trusting the map, whose order depends on serde_json's `preserve_order` feature.
            let mut members: Vec<_> = map.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                path.push(key.clone());
                write_value(item, out, path)?;
                path.pop();
            }
            out.push('}');
        }
    }
    Ok(())
}

pub(crate) fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// The R-TYP-08 rendering of the JSON number `text`, read exactly from its digits; `None` if no `Number` holds it.
pub(crate) fn canonical_number(text: &str) -> Option<String> {
    Number::parse(text).map(|n| n.to_string())
}

/// A JSON Pointer (RFC 6901) for `path`.
pub(crate) fn pointer(path: &[String]) -> String {
    path.iter()
        .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
        .collect()
}

/// The prefix of the object keys serde_json uses, under `arbitrary_precision`, to pass a number's text.
const RESERVED_KEY_PREFIX: &str = "$serde_json::private::";

/// Walks any JSON value and fails at the first rejected object key or depth; `failure` then says why.
struct Walk {
    path: Vec<String>,
    failure: Option<ParseError>,
}

impl Walk {
    fn fail<E: de::Error>(&mut self, failure: ParseError) -> E {
        let error = E::custom(&failure);
        self.failure = Some(failure);
        error
    }

    fn enter<E: de::Error>(&mut self) -> Result<(), E> {
        if self.path.len() >= MAX_JSON_DEPTH {
            return Err(self.fail(ParseError::TooDeep {
                pointer: pointer(&self.path),
            }));
        }
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for &mut Walk {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for &mut Walk {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        self.enter()?;
        for i in 0usize.. {
            self.path.push(i.to_string());
            let more = seq.next_element_seed(&mut *self)?.is_some();
            self.path.pop();
            if !more {
                break;
            }
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let Key::Text(key) = key else {
                // A number's text under `arbitrary_precision`: a leaf, not an object.
                map.next_value::<de::IgnoredAny>()?;
                continue;
            };
            self.enter()?;
            self.path.push(key.clone());
            if key.starts_with(RESERVED_KEY_PREFIX) {
                return Err(self.fail(ParseError::ReservedKey {
                    pointer: pointer(&self.path),
                }));
            }
            if !seen.insert(key) {
                return Err(self.fail(ParseError::DuplicateKey {
                    pointer: pointer(&self.path),
                }));
            }
            map.next_value_seed(&mut *self)?;
            self.path.pop();
        }
        Ok(())
    }
}

/// An object key as the input spells it, or the marker serde_json passes for a number.
enum Key {
    Text(String),
    Number,
}

/// Reads a key as bytes: serde_json hands a written key over as (unescaped) bytes, but its number marker as a `str`,
/// which is how a reserved key in the input is told apart from a real number.
struct KeySeed;

impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Key;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Key, D::Error> {
        deserializer.deserialize_bytes(self)
    }
}

impl Visitor<'_> for KeySeed {
    type Value = Key;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object key")
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Key, E> {
        String::from_utf8(bytes.to_vec())
            .map(Key::Text)
            .map_err(|_| E::custom("object key is not valid Unicode"))
    }

    fn visit_str<E>(self, _: &str) -> Result<Key, E> {
        Ok(Key::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_number;

    #[test]
    fn numbers_render_plain_decimal() {
        for (text, want) in [
            ("0", "0"),
            ("-0", "0"),
            ("-0.000e7", "0"),
            ("0e99999999999999999999", "0"),
            ("820.0", "820"),
            ("2.50", "2.5"),
            ("1e2", "100"),
            ("1E+2", "100"),
            ("1e-3", "0.001"),
            ("-1.5e1", "-15"),
            ("12.34e-1", "1.234"),
            ("0.1", "0.1"),
            ("12.00", "12"),
            ("0.0e-5", "0"),
            ("1e28", "10000000000000000000000000000"),
            ("1e-28", "0.0000000000000000000000000001"),
            ("1.0000000000000000000000000000000", "1"),
            ("79228162514264337593543950335", "79228162514264337593543950335"),
            ("-7.9228162514264337593543950335", "-7.9228162514264337593543950335"),
        ] {
            assert_eq!(canonical_number(text).as_deref(), Some(want), "{text}");
        }
    }

    #[test]
    fn numbers_outside_the_range_have_no_rendering() {
        for text in [
            "79228162514264337593543950336",
            "1e29",
            "1e-29",
            "0.12345678901234567890123456789",
            "1e99999999999999999999",
            "1e-400",
            "",
            "-",
            "1.2.3",
            "1e",
            "0e+",
            "0012.00",
            "1.",
            ".5",
            "1e+-5",
            "+1",
            "-01",
            "1.e5",
            "NaN",
        ] {
            assert_eq!(canonical_number(text), None, "{text}");
        }
    }
}
