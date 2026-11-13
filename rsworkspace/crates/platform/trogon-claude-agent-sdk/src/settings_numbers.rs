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

use serde_json::{Number, Value};

use crate::{
    constants::{
        JS_MAX_EXACT_INTEGER, SETTINGS_INFINITY_MARKER, SETTINGS_INFINITY_MARKER_INTEGER,
        SETTINGS_NEGATIVE_INFINITY_MARKER, SETTINGS_NEGATIVE_INFINITY_MARKER_INTEGER,
    },
    settings_dialect::SettingsDialect,
};

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
    if is_infinity(number) {
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
