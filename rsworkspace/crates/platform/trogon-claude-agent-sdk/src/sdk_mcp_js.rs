//! The JavaScript string, number and key-order semantics the zod port in `sdk_mcp_zod` depends on.

use crate::{JsonObject, settings_json::js_stringify};
use serde_json::Value;

/// `ECMAScript WhiteSpace` and `LineTerminator`, the characters `String.prototype.trim` removes.
fn is_space(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `text.trim()`.
pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `text.length`: the number of UTF-16 code units.
pub(crate) fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The array index a canonical numeric string names; the keys `Object.keys` lists first.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index: u64 = key.parse().ok()?;
    u32::try_from(index).ok().filter(|index| *index != u32::MAX)
}

/// `Object.keys(object)`: integer keys in ascending order, then the others in insertion order.
pub(crate) fn ordered_keys(object: &JsonObject) -> Vec<&String> {
    let mut indexed: Vec<(u32, &String)> = Vec::new();
    let mut named: Vec<&String> = Vec::new();
    for key in object.keys() {
        match array_index(key) {
            Some(index) => indexed.push((index, key)),
            None => named.push(key),
        }
    }
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, key)| key).chain(named).collect()
}

/// The object a JavaScript engine would enumerate: the same entries in `Object.keys` order.
pub(crate) fn ordered(object: JsonObject) -> JsonObject {
    let order: Vec<String> = ordered_keys(&object).into_iter().cloned().collect();
    let mut object = object;
    order
        .into_iter()
        .filter_map(|key| object.remove(&key).map(|value| (key, value)))
        .collect()
}

/// `String(value)` of a finite number.
pub(crate) fn number_text(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    js_string(&Value::from(value))
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
    let text = trim(text);
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

/// `Number.parseInt(text)` for a decimal string.
fn parse_int(text: &str) -> f64 {
    let text = text.trim_start_matches(is_space);
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    sign * digits.parse().unwrap_or(f64::NAN)
}

/// `value.toFixed(digits)`.
fn to_fixed(value: f64, digits: usize) -> String {
    if value.abs() >= 1e21 {
        return number_text(value);
    }
    format!("{value:.digits$}")
}

fn decimals(text: &str) -> usize {
    text.split('.').nth(1).map_or(0, |fraction| fraction.chars().count())
}

/// zod `floatSafeRemainder`: the remainder zod's `multipleOf` check compares with zero.
pub(crate) fn float_safe_remainder(value: f64, step: f64) -> f64 {
    let value_decimals = decimals(&number_text(value));
    let step_text = number_text(step);
    let mut step_decimals = decimals(&step_text);
    if step_decimals == 0
        && let Some(position) = step_text.find("e-")
        && let Some(digit) = step_text
            .get(position + 2..)
            .and_then(|rest| rest.chars().next())
            .and_then(|character| character.to_digit(10))
    {
        step_decimals = digit as usize;
    }
    let places = value_decimals.max(step_decimals);
    let scaled = |number: f64| parse_int(&to_fixed(number, places).replacen('.', "", 1));
    (scaled(value) % scaled(step)) / 10f64.powf(places as f64)
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
