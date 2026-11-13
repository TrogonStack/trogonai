//! JavaScript number semantics for parsed settings documents.
//!
//! `JSON.parse` yields doubles and `JSON.stringify` prints the shortest digits that identify the double, so a
//! whole number prints without a fraction and an integer above 2^53 prints rounded, with zeros after the
//! significant digits. `serde_json` prints an integer exactly and a float with a fraction, so a whole double is
//! stored as the integer JavaScript prints. That decimal parses back to the same double.

use serde_json::{Number, Value};

use crate::{constants::JS_MAX_EXACT_INTEGER, settings_dialect::SettingsDialect};

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
