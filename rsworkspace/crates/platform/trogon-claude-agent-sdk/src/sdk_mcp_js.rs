//! The JavaScript string and number semantics the zod port in `sdk_mcp_zod` depends on.

use crate::{session_prompt::js_trim, settings_json::js_stringify};
use serde_json::Value;

/// `text.length`: the number of UTF-16 code units.
pub(crate) fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The `Number(value)` conversion of a JSON value, as the relational operators apply it.
pub(crate) fn to_number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Null => 0.0,
        Value::String(text) => string_to_number(text),
        Value::Array(_) => string_to_number(&js_string(value)),
        Value::Object(_) => f64::NAN,
    }
}

fn string_to_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix)
                .map(|number| number as f64)
                .unwrap_or(f64::NAN);
        }
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if !text
        .chars()
        .all(|character| character.is_ascii_digit() || matches!(character, '.' | 'e' | 'E' | '+' | '-'))
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// zod `floatSafeRemainder`: the remainder zod's `multipleOf` check compares with zero. The quotient counts as
/// whole when it is within four `Number.EPSILON` of the nearest integer, scaled by the quotient when above one.
pub(crate) fn float_safe_remainder(value: f64, step: f64) -> f64 {
    let quotient = value / step;
    let nearest = js_round(quotient);
    let tolerance = 4.0 * f64::EPSILON * quotient.abs().max(1.0);
    if (quotient - nearest).abs() < tolerance {
        return 0.0;
    }
    quotient - nearest
}

/// `Math.round(number)`: a half rounds up, toward positive infinity.
fn js_round(number: f64) -> f64 {
    let nearest = number.round();
    if number - nearest == 0.5 {
        nearest + 1.0
    } else {
        nearest
    }
}

/// Whether `atob(text)` returns instead of throwing: the forgiving-base64 decode of the HTML standard.
pub(crate) fn forgiving_base64(text: &str) -> bool {
    let mut data: Vec<char> = text
        .chars()
        .filter(|character| !matches!(character, '\t' | '\n' | '\u{c}' | '\r' | ' '))
        .collect();
    if matches!(data.len() % 4, 0) {
        for _ in 0..2 {
            if data.last() == Some(&'=') {
                data.pop();
            }
        }
    }
    data.len() % 4 != 1
        && data
            .iter()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '/'))
}

/// The `length` of the function `Object.prototype[key]` is, for a key a plain object inherits.
pub(crate) fn prototype_function_length(key: &str) -> Option<usize> {
    match key {
        "constructor"
        | "hasOwnProperty"
        | "isPrototypeOf"
        | "propertyIsEnumerable"
        | "__lookupGetter__"
        | "__lookupSetter__" => Some(1),
        "toString" | "toLocaleString" | "valueOf" => Some(0),
        "__defineGetter__" | "__defineSetter__" => Some(2),
        _ => None,
    }
}

/// zod `stringifyPrimitive`: a string in double quotes without escaping, anything else as `String(value)`.
pub(crate) fn stringify_primitive(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => js_string(other),
    }
}

/// `String(value)` for a JSON value; a `null`, boolean or number reads as its `JSON.stringify` text.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
        scalar => js_stringify(scalar),
    }
}
