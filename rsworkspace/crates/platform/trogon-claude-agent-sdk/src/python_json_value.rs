//! JSON values as Python's `json.loads` reads them and `json.dumps` writes them. An integer token stays an exact
//! integer of any size, which `serde_json::Value` cannot hold without its `arbitrary_precision` feature, a float
//! token stays a float even when it is whole (`1.0` prints back as `1.0`), `NaN`, `Infinity` and `-Infinity`
//! read and print as Python spells them, and a string keeps a lone surrogate.

use std::{
    fmt::{self, Write as _},
    ops::Index,
    str::FromStr,
};

use indexmap::IndexMap;
use serde_json::{Map, Number, Value};

use crate::python_json_error::ExactIntegerError;
use crate::{constants::PYTHON_INT_MAX_STR_DIGITS, python_json::float_repr, python_string::PythonString};

/// A Python `int` of any size, held as its decimal digits.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ExactInteger {
    /// Canonical decimal: an optional `-`, then digits without leading zeros; zero is `0`.
    digits: String,
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
    /// Whether `json.loads` read it as a `float`.
    pub fn is_f64(&self) -> bool {
        matches!(self, Self::Float(_))
    }

    /// The `int` as an `i64`, or `None` for a `float` or an `int` outside the `i64` range.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(integer) => integer.to_i64(),
            Self::Float(_) => None,
        }
    }

    /// The `int` as a `u64`, or `None` for a `float` or an `int` outside the `u64` range.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(integer) => integer.to_u64(),
            Self::Float(_) => None,
        }
    }

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
///
/// Dropping, cloning, comparing and printing it with `{:?}` or `{:#?}` run without recursing, so a value as deep
/// as [`python_json_loads`](crate::python_json_loads) reads needs no more stack than a shallow one. Because it
/// implements [`Drop`], a match cannot move an array, object, string or number out of it: match on a reference and
/// [`std::mem::take`] the member instead.
pub enum PythonJsonValue {
    Null,
    Bool(bool),
    Number(PythonNumber),
    String(PythonString),
    Array(Vec<PythonJsonValue>),
    Object(PythonJsonObject),
}

/// The members of a JSON object, each key at its first position with its last value, as a `dict` keeps them.
pub type PythonJsonObject = IndexMap<PythonString, PythonJsonValue>;

impl PythonJsonValue {
    /// The member named `key`, or `None` when it is missing or this is not an object.
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?.get(key)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&PythonNumber> {
        match self {
            Self::Number(number) => Some(number),
            _ => None,
        }
    }

    pub fn as_python_str(&self) -> Option<&PythonString> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// The string as a `&str`, or `None` when this is not a string or the string holds a surrogate.
    pub fn as_str(&self) -> Option<&str> {
        self.as_python_str()?.as_str()
    }

    pub fn as_array(&self) -> Option<&Vec<Self>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&PythonJsonObject> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut PythonJsonObject> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }
}

/// `value[key]` as `serde_json::Value` indexes: the member, or `null` when it is missing or this is not an object.
impl Index<&str> for PythonJsonValue {
    type Output = Self;

    fn index(&self, key: &str) -> &Self {
        static NULL: PythonJsonValue = PythonJsonValue::Null;
        self.get(key).unwrap_or(&NULL)
    }
}

/// Converts without recursing, so a `serde_json::Value` of any depth converts without exhausting the stack.
impl From<&Value> for PythonJsonValue {
    fn from(value: &Value) -> Self {
        let mut open = Vec::new();
        let mut next = value;
        loop {
            let mut built = match next {
                Value::Null => Some(Self::Null),
                Value::Bool(flag) => Some(Self::Bool(*flag)),
                Value::Number(number) => Some(Self::Number(number.into())),
                Value::String(text) => Some(Self::String(text.as_str().into())),
                Value::Array(items) => {
                    open.push(Open::Array(Vec::with_capacity(items.len()), items.iter()));
                    None
                }
                Value::Object(members) => {
                    let filled = PythonJsonObject::with_capacity(members.len());
                    open.push(Open::Object(filled, members.iter(), PythonString::new()));
                    None
                }
            };
            loop {
                let Some(top) = open.last_mut() else {
                    return built.unwrap_or(Self::Null);
                };
                if let Some(child) = built.take() {
                    top.attach(child);
                }
                match top.advance() {
                    Some(child) => {
                        next = child;
                        break;
                    }
                    None => built = open.pop().map(Open::close),
                }
            }
        }
    }
}

/// A container the conversion from a `serde_json::Value` is still filling, with the members of the source it has yet to visit and,
/// for an object, the key of the member it visits.
enum Open<'v> {
    Array(Vec<PythonJsonValue>, std::slice::Iter<'v, Value>),
    Object(PythonJsonObject, serde_json::map::Iter<'v>, PythonString),
}

impl<'v> Open<'v> {
    /// Stores `child`, converted from the member [`Open::advance`] returned last.
    fn attach(&mut self, child: PythonJsonValue) {
        match self {
            Self::Array(items, _) => items.push(child),
            Self::Object(members, _, key) => {
                members.insert(std::mem::take(key), child);
            }
        }
    }

    /// The next member to convert, or `None` once every member is stored.
    fn advance(&mut self) -> Option<&'v Value> {
        match self {
            Self::Array(_, rest) => rest.next(),
            Self::Object(_, rest, key) => rest.next().map(|(name, child)| {
                *key = name.as_str().into();
                child
            }),
        }
    }

    fn close(self) -> PythonJsonValue {
        match self {
            Self::Array(items, _) => PythonJsonValue::Array(items),
            Self::Object(members, _, _) => PythonJsonValue::Object(members),
        }
    }
}

/// An object converted member by member, each without recursing.
impl From<&Map<String, Value>> for PythonJsonValue {
    fn from(members: &Map<String, Value>) -> Self {
        Self::Object(
            members
                .iter()
                .map(|(name, member)| (PythonString::from(name.as_str()), Self::from(member)))
                .collect(),
        )
    }
}

impl From<Value> for PythonJsonValue {
    fn from(value: Value) -> Self {
        Self::from(&value)
    }
}

impl From<bool> for PythonJsonValue {
    fn from(flag: bool) -> Self {
        Self::Bool(flag)
    }
}

impl From<i64> for PythonJsonValue {
    fn from(integer: i64) -> Self {
        Self::Number(PythonNumber::Int(integer.into()))
    }
}

impl From<i32> for PythonJsonValue {
    fn from(integer: i32) -> Self {
        Self::from(i64::from(integer))
    }
}

impl From<u64> for PythonJsonValue {
    fn from(integer: u64) -> Self {
        Self::Number(PythonNumber::Int(integer.into()))
    }
}

impl From<usize> for PythonJsonValue {
    fn from(integer: usize) -> Self {
        Self::Number(PythonNumber::Int(ExactInteger {
            digits: integer.to_string(),
        }))
    }
}

/// A `float`, `NaN` and the infinities included, as Python holds them.
impl From<f64> for PythonJsonValue {
    fn from(float: f64) -> Self {
        Self::Number(PythonNumber::Float(float))
    }
}

impl From<&str> for PythonJsonValue {
    fn from(text: &str) -> Self {
        Self::String(text.into())
    }
}

/// Python `json.dumps(value)` with its defaults: `", "` and `": "` separators, every character outside printable
/// ASCII escaped as `\uXXXX`, floats spelled by `repr`, and `NaN`, `Infinity` and `-Infinity` for the non-finite
/// floats.
///
/// It writes without recursing, so a value as deep as [`python_json_loads`](crate::python_json_loads) reads cannot
/// exhaust the stack.
pub fn python_json_dumps(value: &PythonJsonValue) -> String {
    let mut out = String::new();
    let mut pending = vec![Pending::Value(value)];
    while let Some(next) = pending.pop() {
        match next {
            Pending::Text(text) => out.push_str(text),
            Pending::Key(key) => write_string(key, &mut out),
            Pending::Value(value) => write_value(value, &mut out, &mut pending),
        }
    }
    out
}

/// What [`python_json_dumps`] still has to write, last first.
enum Pending<'a> {
    Value(&'a PythonJsonValue),
    Key(&'a PythonString),
    Text(&'static str),
}

/// Writes a scalar, or the opening bracket of an array or object with the rest of it pushed onto `pending`.
fn write_value<'a>(value: &'a PythonJsonValue, out: &mut String, pending: &mut Vec<Pending<'a>>) {
    match value {
        PythonJsonValue::Null => out.push_str("null"),
        PythonJsonValue::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        PythonJsonValue::Number(PythonNumber::Int(integer)) => out.push_str(integer.as_str()),
        PythonJsonValue::Number(PythonNumber::Float(float)) => out.push_str(&float_repr(*float)),
        PythonJsonValue::String(text) => write_string(text, out),
        PythonJsonValue::Array(items) => {
            out.push('[');
            pending.push(Pending::Text("]"));
            for (position, item) in items.iter().enumerate().rev() {
                pending.push(Pending::Value(item));
                if position > 0 {
                    pending.push(Pending::Text(", "));
                }
            }
        }
        PythonJsonValue::Object(entries) => {
            out.push('{');
            pending.push(Pending::Text("}"));
            for (position, (key, item)) in entries.iter().enumerate().rev() {
                pending.push(Pending::Value(item));
                pending.push(Pending::Text(": "));
                pending.push(Pending::Key(key));
                if position > 0 {
                    pending.push(Pending::Text(", "));
                }
            }
        }
    }
}

/// `json.encoder.py_encode_basestring_ascii`, which writes a lone surrogate as its own `\uXXXX` escape.
fn write_string(text: &PythonString, out: &mut String) {
    out.push('"');
    for code_point in text.code_points() {
        match code_point.as_char() {
            Some('"') => out.push_str("\\\""),
            Some('\\') => out.push_str("\\\\"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some('\t') => out.push_str("\\t"),
            Some('\u{8}') => out.push_str("\\b"),
            Some('\u{c}') => out.push_str("\\f"),
            Some(ch @ ' '..='~') => out.push(ch),
            Some(ch) => {
                let mut units = [0_u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            None => {
                let _ = write!(out, "\\u{:04x}", code_point.as_u32());
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

#[path = "python_json_value_deep.rs"]
mod deep;

#[cfg(test)]
#[path = "python_json_value_tests.rs"]
mod tests;
