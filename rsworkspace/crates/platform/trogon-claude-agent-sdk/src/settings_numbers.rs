//! JavaScript number semantics for parsed settings documents.
//!
//! `JSON.parse` yields doubles and `JSON.stringify` prints the shortest digits that identify the double, so a
//! whole number prints without a fraction and an integer above 2^53 prints rounded, with zeros after the
//! significant digits. `serde_json` prints an integer exactly and a float with a fraction, so a whole double is
//! stored as the integer JavaScript prints. That decimal parses back to the same double.
//!
//! `JSON.parse` reads a token past the double range as `Infinity`, which `serde_json` cannot hold. A settings
//! value carries it as a marker double that normalization never leaves behind, the number sites read the marker
//! back as the infinity it stands for, and the resolver prints it as `null`, as `JSON.stringify` does.
//!
//! `json.loads` reads an integer of any size exactly and keeps a whole float a float, which `serde_json` cannot
//! hold either. A Python-dialect resolve keeps such numbers on its thread, each as a marker double, and turns the
//! markers back into the exact numbers, or into their nearest `serde_json` numbers, when it ends.

use std::cell::RefCell;

use serde_json::{Number, Value};

use crate::{
    constants::{
        JS_MAX_EXACT_INTEGER, SETTINGS_INFINITY_MARKER, SETTINGS_INFINITY_MARKER_INTEGER,
        SETTINGS_NEGATIVE_INFINITY_MARKER, SETTINGS_NEGATIVE_INFINITY_MARKER_INTEGER,
        SETTINGS_PYTHON_NUMBER_MARKER_BASE, SETTINGS_PYTHON_NUMBER_MARKER_STEP,
    },
    python_json_value::{PythonJsonValue, PythonNumber},
    settings_dialect::SettingsDialect,
};

thread_local! {
    static PYTHON_NUMBERS: RefCell<Option<Vec<PythonNumber>>> = const { RefCell::new(None) };
}

/// Keeps the exact numbers of a Python-dialect resolve on this thread until it is dropped.
pub(crate) struct PythonNumbers {
    previous: Option<Vec<PythonNumber>>,
}

impl PythonNumbers {
    pub(crate) fn start() -> Self {
        Self {
            previous: PYTHON_NUMBERS.replace(Some(Vec::new())),
        }
    }
}

impl Drop for PythonNumbers {
    fn drop(&mut self) {
        PYTHON_NUMBERS.set(self.previous.take());
    }
}

/// The settings value for a number `json.loads` read. An integer that fits 64 bits and a float with a fraction
/// are plain `serde_json` numbers; any other number is a marker while a Python-dialect resolve keeps numbers, and
/// its nearest `serde_json` value otherwise.
pub(crate) fn python_number(number: PythonNumber) -> Value {
    match &number {
        PythonNumber::Int(integer) => {
            if let Some(value) = integer.to_u64() {
                return Value::from(value);
            }
            if let Some(value) = integer.to_i64() {
                return Value::from(value);
            }
        }
        PythonNumber::Float(float) if float.is_finite() && float.fract() != 0.0 => {
            return Number::from_f64(*float).map_or(Value::Null, Value::Number);
        }
        PythonNumber::Float(_) => {}
    }
    let marker = PYTHON_NUMBERS.with_borrow_mut(|numbers| {
        let numbers = numbers.as_mut()?;
        let index = match numbers.iter().position(|kept| kept.is_identical(&number)) {
            Some(index) => index,
            None => {
                numbers.push(number.clone());
                numbers.len() - 1
            }
        };
        let marker = SETTINGS_PYTHON_NUMBER_MARKER_BASE + SETTINGS_PYTHON_NUMBER_MARKER_STEP * index as f64;
        (marker < SETTINGS_INFINITY_MARKER)
            .then(|| Number::from_f64(marker))
            .flatten()
    });
    marker.map_or_else(|| approximate(&number), Value::Number)
}

/// The settings value for a JSON value `json.loads` read.
pub(crate) fn python_value(value: PythonJsonValue) -> Value {
    match value {
        PythonJsonValue::Null => Value::Null,
        PythonJsonValue::Bool(flag) => Value::Bool(flag),
        PythonJsonValue::Number(number) => python_number(number),
        PythonJsonValue::String(text) => Value::String(text),
        PythonJsonValue::Array(items) => Value::Array(items.into_iter().map(python_value).collect()),
        PythonJsonValue::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, entry)| (key, python_value(entry)))
                .collect(),
        ),
    }
}

/// The exact number a marker stands for, while the resolve that made it keeps numbers.
fn kept_number(number: &Number) -> Option<PythonNumber> {
    if !number.is_f64() {
        return None;
    }
    let value = number.as_f64()?;
    let offset = (value - SETTINGS_PYTHON_NUMBER_MARKER_BASE) / SETTINGS_PYTHON_NUMBER_MARKER_STEP;
    if !(0.0..).contains(&offset) || offset.fract() != 0.0 || value >= SETTINGS_INFINITY_MARKER {
        return None;
    }
    PYTHON_NUMBERS.with_borrow(|numbers| numbers.as_ref()?.get(offset as usize).cloned())
}

/// The nearest `serde_json` value: the double of the number, or `null` for one that is not finite.
fn approximate(number: &PythonNumber) -> Value {
    Number::from_f64(number.to_f64()).map_or(Value::Null, Value::Number)
}

/// Replaces every kept-number marker of `value` with its nearest `serde_json` value.
pub(crate) fn approximate_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(kept) = kept_number(number) {
                *value = approximate(&kept);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(approximate_numbers),
        Value::Object(entries) => entries.values_mut().for_each(approximate_numbers),
        _ => {}
    }
}

/// `value` with every marker turned back into the number it stands for: a kept number, or an infinity.
pub(crate) fn exact_numbers(value: &Value) -> PythonJsonValue {
    match value {
        Value::Number(number) => PythonJsonValue::Number(kept_number(number).unwrap_or_else(|| {
            match js_value(number).filter(|value| value.is_infinite()) {
                Some(infinite) => PythonNumber::Float(infinite),
                None => PythonNumber::from(number),
            }
        })),
        Value::Array(items) => PythonJsonValue::Array(items.iter().map(exact_numbers).collect()),
        Value::Object(entries) => PythonJsonValue::Object(
            entries
                .iter()
                .map(|(key, entry)| (key.clone(), exact_numbers(entry)))
                .collect(),
        ),
        other => PythonJsonValue::from(other),
    }
}

/// The settings number that stands for `Infinity` or, when `negative`, `-Infinity`.
pub(crate) fn infinity(negative: bool) -> Number {
    let marker = if negative {
        SETTINGS_NEGATIVE_INFINITY_MARKER
    } else {
        SETTINGS_INFINITY_MARKER
    };
    Number::from_f64(marker).unwrap_or_else(|| Number::from(0_u64))
}

/// Whether `value` is a finite double with the value of an infinity marker, which a settings number must not hold.
pub(crate) fn is_marker_value(value: f64) -> bool {
    value.to_bits() == SETTINGS_INFINITY_MARKER.to_bits()
        || value.to_bits() == SETTINGS_NEGATIVE_INFINITY_MARKER.to_bits()
}

/// The settings value for a finite double, `null` for `NaN`. A double equal to an infinity marker is held as the
/// equal integer, which every reader takes for the same double, so it keeps the value it has upstream.
pub(crate) fn finite_number(value: f64) -> Value {
    if value.to_bits() == SETTINGS_INFINITY_MARKER.to_bits() {
        return Value::from(SETTINGS_INFINITY_MARKER_INTEGER);
    }
    if value.to_bits() == SETTINGS_NEGATIVE_INFINITY_MARKER.to_bits() {
        return Value::from(SETTINGS_NEGATIVE_INFINITY_MARKER_INTEGER);
    }
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// The JavaScript number a settings number stands for, with the infinity markers read back as infinities.
pub(crate) fn js_value(number: &Number) -> Option<f64> {
    if let Some(kept) = kept_number(number) {
        return Some(kept.to_f64());
    }
    let value = number.as_f64()?;
    if !number.is_f64() {
        return Some(value);
    }
    Some(if value.to_bits() == SETTINGS_INFINITY_MARKER.to_bits() {
        f64::INFINITY
    } else if value.to_bits() == SETTINGS_NEGATIVE_INFINITY_MARKER.to_bits() {
        f64::NEG_INFINITY
    } else {
        value
    })
}

/// JavaScript `SameValueZero`, the equality a `Set` keeps primitives by: numbers compare by the double they
/// stand for, so `1` and `1.0`, `0` and `-0`, or a kept number and its double are one value, and `NaN` is one.
pub(crate) fn same_value_zero(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => js_value(left)
            .zip(js_value(right))
            .is_some_and(|(left, right)| left == right || (left.is_nan() && right.is_nan())),
        _ => left == right,
    }
}

fn is_infinity(number: &Number) -> bool {
    js_value(number).is_some_and(f64::is_infinite)
}

/// Replaces every infinity of `value` with `null`, the value `JSON.stringify` prints for it.
pub(crate) fn infinities_to_null(value: &mut Value) {
    match value {
        Value::Number(number) if is_infinity(number) => *value = Value::Null,
        Value::Array(items) => items.iter_mut().for_each(infinities_to_null),
        Value::Object(entries) => entries.values_mut().for_each(infinities_to_null),
        _ => {}
    }
}

/// Rewrites every number of `value` so that `serde_json` prints what the dialect's JSON writer prints.
pub(crate) fn normalize_numbers(value: &mut Value, dialect: SettingsDialect) {
    match value {
        Value::Number(number) => {
            if let Some(normalized) = normalized_number(number, dialect) {
                *number = normalized;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| normalize_numbers(item, dialect)),
        Value::Object(entries) => entries.values_mut().for_each(|entry| normalize_numbers(entry, dialect)),
        _ => {}
    }
}

fn normalized_number(number: &Number, dialect: SettingsDialect) -> Option<Number> {
    if is_infinity(number) || kept_number(number).is_some() {
        return None;
    }
    if number.is_i64() || number.is_u64() {
        let magnitude = number.as_i64().map(i64::unsigned_abs).or_else(|| number.as_u64());
        if dialect == SettingsDialect::Python || magnitude.is_some_and(|magnitude| magnitude <= JS_MAX_EXACT_INTEGER) {
            return None;
        }
    }
    let float = number.as_f64()?;
    if !float.is_finite() || float.fract() != 0.0 {
        return None;
    }
    if float == 0.0 {
        return Some(Number::from(0_u64));
    }
    // `Display` for `f64` prints the shortest round-trip digits, padded with zeros, never an exponent.
    let digits = float.to_string();
    let integer = if float > 0.0 {
        digits.parse::<u64>().ok().map(Number::from)
    } else {
        digits.parse::<i64>().ok().map(Number::from)
    };
    integer.or_else(|| Number::from_f64(float))
}

#[cfg(test)]
#[path = "settings_numbers_tests.rs"]
mod tests;
