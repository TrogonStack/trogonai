//! JSON values as Python's `json.loads` reads them and `json.dumps` writes them. An integer token stays an exact
//! integer of any size, which `serde_json::Value` cannot hold without its `arbitrary_precision` feature, a float
//! token stays a float even when it is whole (`1.0` prints back as `1.0`), and `NaN`, `Infinity` and `-Infinity`
//! read and print as Python spells them.

use std::{
    cell::Cell,
    fmt::{self, Write as _},
    str::FromStr,
};

use indexmap::IndexMap;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Number, Value};

use crate::{
    constants::PYTHON_INT_MAX_STR_DIGITS,
    json_scan::{Token, scan},
    python_json::float_repr,
};

/// A Python `int` of any size, held as its decimal digits.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ExactInteger {
    /// Canonical decimal: an optional `-`, then digits without leading zeros; zero is `0`.
    digits: String,
}

/// Why text is not an [`ExactInteger`]. The messages are the ones Python's `int()` raises.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ExactIntegerError {
    #[error("invalid literal for int() with base 10: {0}")]
    Invalid(String),
    #[error(
        "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    )]
    TooManyDigits { limit: usize, digits: usize },
}

impl ExactInteger {
    /// The canonical decimal spelling: an optional `-` and digits without leading zeros.
    pub fn as_str(&self) -> &str {
        &self.digits
    }

    pub fn is_negative(&self) -> bool {
        self.digits.starts_with('-')
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.digits.parse().ok()
    }

    pub fn to_u64(&self) -> Option<u64> {
        self.digits.parse().ok()
    }

    /// The nearest double, as Python's `float(n)` rounds, except that a value past the double range reads as an
    /// infinity where Python raises `OverflowError`.
    pub fn to_f64(&self) -> f64 {
        self.digits.parse().unwrap_or(f64::NAN)
    }
}

impl FromStr for ExactInteger {
    type Err = ExactIntegerError;

    /// Python `int(text)` for an optionally signed run of ASCII decimal digits, with Python's default limit of
    /// 4300 digits.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(digits) => (true, digits),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ExactIntegerError::Invalid(python_str_repr(text)));
        }
        if digits.len() > PYTHON_INT_MAX_STR_DIGITS {
            return Err(ExactIntegerError::TooManyDigits {
                limit: PYTHON_INT_MAX_STR_DIGITS,
                digits: digits.len(),
            });
        }
        let significant = digits.trim_start_matches('0');
        let digits = match (significant.is_empty(), negative) {
            (true, _) => "0".to_owned(),
            (false, true) => format!("-{significant}"),
            (false, false) => significant.to_owned(),
        };
        Ok(Self { digits })
    }
}

impl fmt::Display for ExactInteger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.digits)
    }
}

impl From<i64> for ExactInteger {
    fn from(value: i64) -> Self {
        Self {
            digits: value.to_string(),
        }
    }
}

impl From<u64> for ExactInteger {
    fn from(value: u64) -> Self {
        Self {
            digits: value.to_string(),
        }
    }
}

/// A Python number as `json.loads` returns it.
#[derive(Clone, Debug, PartialEq)]
pub enum PythonNumber {
    Int(ExactInteger),
    Float(f64),
}

impl PythonNumber {
    /// The nearest double; see [`ExactInteger::to_f64`].
    pub fn to_f64(&self) -> f64 {
        match self {
            Self::Int(integer) => integer.to_f64(),
            Self::Float(float) => *float,
        }
    }

    /// Whether both are the same value of the same type, a float compared by its bits, so that `0.0` and `-0.0`
    /// differ and `NaN` equals itself.
    pub(crate) fn is_identical(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            _ => false,
        }
    }
}

impl From<&Number> for PythonNumber {
    fn from(number: &Number) -> Self {
        if let Some(value) = number.as_u64() {
            Self::Int(value.into())
        } else if let Some(value) = number.as_i64() {
            Self::Int(value.into())
        } else {
            Self::Float(number.as_f64().unwrap_or(f64::NAN))
        }
    }
}

/// A JSON value as Python's `json.loads` returns it. An object keeps the position of a key's first occurrence and
/// the value of its last, as a `dict` does.
#[derive(Clone, Debug, PartialEq)]
pub enum PythonJsonValue {
    Null,
    Bool(bool),
    Number(PythonNumber),
    String(String),
    Array(Vec<PythonJsonValue>),
    Object(IndexMap<String, PythonJsonValue>),
}

impl From<&Value> for PythonJsonValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(*flag),
            Value::Number(number) => Self::Number(number.into()),
            Value::String(text) => Self::String(text.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from).collect()),
            Value::Object(entries) => Self::Object(
                entries
                    .iter()
                    .map(|(key, entry)| (key.clone(), Self::from(entry)))
                    .collect(),
            ),
        }
    }
}

impl From<Value> for PythonJsonValue {
    fn from(value: Value) -> Self {
        Self::from(&value)
    }
}

/// Why [`python_json_loads`] refused text.
#[derive(Debug, thiserror::Error)]
pub enum PythonJsonError {
    /// The text is not JSON. The message is `serde_json`'s, not the `JSONDecodeError` text Python prints.
    #[error("{0}")]
    Syntax(#[source] serde_json::Error),
    /// The text starts with a byte order mark, which `json.loads` refuses for a `str`.
    #[error("Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)")]
    ByteOrderMark,
    /// An integer token has more digits than Python converts.
    #[error("{0}")]
    Integer(#[source] ExactIntegerError),
}

/// Python `json.loads(text)`.
///
/// It accepts what Python accepts except for a `\ud800`-style escape of a lone surrogate, which a Rust `String`
/// cannot hold, and arrays or objects nested deeper than 128 levels, where `serde_json` stops.
pub fn python_json_loads(text: &str) -> Result<PythonJsonValue, PythonJsonError> {
    if text.starts_with('\u{feff}') {
        return Err(PythonJsonError::ByteOrderMark);
    }
    let scanned = scan(text, true);
    let next = Cell::new(0);
    let failure = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(&scanned.text);
    let read = Exact {
        tokens: &scanned.tokens,
        next: &next,
        failure: &failure,
    }
    .deserialize(&mut deserializer)
    .and_then(|value| deserializer.end().map(|()| value));
    match (read, failure.take()) {
        (_, Some(error)) => Err(PythonJsonError::Integer(error)),
        (Ok(value), None) => Ok(value),
        (Err(error), None) => Err(PythonJsonError::Syntax(error)),
    }
}

/// Python `json.dumps(value)` with its defaults: `", "` and `": "` separators, every character outside printable
/// ASCII escaped as `\uXXXX`, floats spelled by `repr`, and `NaN`, `Infinity` and `-Infinity` for the non-finite
/// floats.
pub fn python_json_dumps(value: &PythonJsonValue) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &PythonJsonValue, out: &mut String) {
    match value {
        PythonJsonValue::Null => out.push_str("null"),
        PythonJsonValue::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        PythonJsonValue::Number(PythonNumber::Int(integer)) => out.push_str(integer.as_str()),
        PythonJsonValue::Number(PythonNumber::Float(float)) => out.push_str(&float_repr(*float)),
        PythonJsonValue::String(text) => write_string(text, out),
        PythonJsonValue::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push(']');
        }
        PythonJsonValue::Object(entries) => {
            out.push('{');
            for (position, (key, item)) in entries.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                write_string(key, out);
                out.push_str(": ");
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

/// `json.encoder.py_encode_basestring_ascii`.
fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            _ => {
                let mut units = [0_u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// Python `repr(text)` for the text of an `int()` error, which quotes with `'` unless the text holds one.
fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch == quote => {
                out.push('\\');
                out.push(ch);
            }
            ch if ch.is_control() => {
                let _ = write!(out, "\\x{:02x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out.push(quote);
    out
}

/// Builds the value `json.loads` would. `serde_json` visits values in text order, so the n-th number or `null`
/// visited is the n-th token of the scan, whose spelling decides between an exact `int` and a `float`.
#[derive(Clone, Copy)]
struct Exact<'a> {
    tokens: &'a [Token<'a>],
    next: &'a Cell<usize>,
    failure: &'a Cell<Option<ExactIntegerError>>,
}

impl<'a> Exact<'a> {
    fn token(self) -> Option<Token<'a>> {
        let index = self.next.get();
        self.next.set(index + 1);
        self.tokens.get(index).copied()
    }

    fn number<E: serde::de::Error>(self) -> Result<PythonJsonValue, E> {
        let Some(Token::Number(spelling)) = self.token() else {
            return Err(E::custom("a number token is missing from the scan"));
        };
        if spelling.contains(['.', 'e', 'E']) {
            let float = spelling.parse::<f64>().map_err(E::custom)?;
            return Ok(PythonJsonValue::Number(PythonNumber::Float(float)));
        }
        match spelling.parse::<ExactInteger>() {
            Ok(integer) => Ok(PythonJsonValue::Number(PythonNumber::Int(integer))),
            Err(error) => {
                let message = error.to_string();
                self.failure.set(Some(error));
                Err(E::custom(message))
            }
        }
    }
}

impl<'de> DeserializeSeed<'de> for Exact<'_> {
    type Value = PythonJsonValue;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<PythonJsonValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Exact<'_> {
    type Value = PythonJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<PythonJsonValue, E> {
        Ok(PythonJsonValue::Bool(value))
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<PythonJsonValue, E> {
        self.number()
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<PythonJsonValue, E> {
        self.number()
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<PythonJsonValue, E> {
        self.number()
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<PythonJsonValue, E> {
        Ok(PythonJsonValue::String(value.to_owned()))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<PythonJsonValue, E> {
        match self.token() {
            Some(Token::Constant(constant)) => Ok(PythonJsonValue::Number(PythonNumber::Float(constant))),
            Some(Token::Null) => Ok(PythonJsonValue::Null),
            _ => Err(E::custom("a null token is missing from the scan")),
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<PythonJsonValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = access.next_element_seed(self)? {
            items.push(item);
        }
        Ok(PythonJsonValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<PythonJsonValue, A::Error> {
        let mut entries = IndexMap::new();
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value_seed(self)?;
            entries.insert(key, value);
        }
        Ok(PythonJsonValue::Object(entries))
    }
}

#[cfg(test)]
#[path = "python_json_value_tests.rs"]
mod tests;
