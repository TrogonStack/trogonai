//! The two JSON writers settings travel through: TypeScript `JSON.stringify` and Python `json.dumps`.
//!
//! They differ from `serde_json` where it matters for byte-identical command lines: JavaScript lists
//! array-index keys first and prints a whole-valued float without a fraction, and Python separates with
//! `", "` and `": "` and escapes every character outside printable ASCII.

use serde_json::{Number, Value};

use crate::{constants::JS_MAX_EXACT_INTEGER, js_object_order::js_key_order};

/// JavaScript `Number::toString` for the double a JSON number token parses to.
fn js_number(number: &Number) -> String {
    let exact = number.as_u64().or_else(|| number.as_i64().map(i64::unsigned_abs));
    if exact.is_some_and(|magnitude| magnitude <= JS_MAX_EXACT_INTEGER) {
        return number.to_string();
    }
    number.as_f64().map_or_else(|| "null".to_owned(), js_float)
}

/// The shortest digits that read back as a positive finite `value`, closest to it and even on a tie, with the
/// position of the decimal point among them. `Number::toString` and `ryu`, which `serde_json` prints doubles
/// with, break a tie to the even digit; Rust's `{:e}` rounds it up, so 2^-25 would end in 3 instead of 2.
fn shortest_digits(value: f64) -> (String, i32) {
    let printed = Value::from(value).to_string();
    let (mantissa, exponent) = printed.split_once('e').unwrap_or((printed.as_str(), "0"));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let leading = digits.len() - digits.trim_start_matches('0').len();
    let point = whole.len() as i32 + exponent.parse::<i32>().unwrap_or(0) - leading as i32;
    (digits.trim_matches('0').to_owned(), point)
}

/// `Number.prototype.toString` for a double: the shortest digits that read back as the same double, in fixed
/// notation from 1e-6 up to 1e21 and in exponent form outside it. A non-finite double prints as `JSON.stringify`
/// prints it, `null`.
pub(crate) fn js_float(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let (digits, point) = shortest_digits(value.abs());
    let count = digits.len() as i32;
    let body = if count <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let power = point - 1;
        let sign_char = if power < 0 { '-' } else { '+' };
        if count == 1 {
            format!("{digits}e{sign_char}{}", power.abs())
        } else {
            format!("{}.{}e{sign_char}{}", &digits[..1], &digits[1..], power.abs())
        }
    };
    format!("{sign}{body}")
}

/// A string literal as `JSON.stringify` writes it: the quote, the backslash and the control characters escaped,
/// the short forms where JSON has one, and every other character as is.
fn js_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            _ => out.push(ch),
        }
    }
    out.push('"');
}

fn js_write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&js_number(number)),
        Value::String(text) => js_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                js_write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (position, (key, item)) in js_key_order(map).into_iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                js_string(key, out);
                out.push(':');
                js_write(item, out);
            }
            out.push('}');
        }
    }
}

/// The text TypeScript's `JSON.stringify(value)` prints for `value`, byte for byte.
///
/// Use it to serialize resolved settings (or any JSON the SDK shares with TypeScript) exactly as the TypeScript SDK
/// would. `serde_json::to_string` keeps insertion order for every key and prints some doubles differently: whole
/// numbers from 2^64 up and at or below -2^63, and fractions from 1e-6 up to 1e-5, come out in exponent form there.
/// This writer follows JavaScript instead:
///
/// - Numbers print as the double a JavaScript number holds, with the shortest digits that read back as that double,
///   in fixed notation from 1e-6 up to 1e21 and as `1e+21`, `1.5e-7` outside it. Integers above 2^53 print as the
///   double they round to, and negative zero prints as `0`.
/// - Object keys that are array indices (`"0"` up to `"4294967294"` without leading zeros) come first in ascending
///   order, then the other keys in insertion order.
/// - Strings escape `"`, `\` and the control characters below U+0020 (`\b`, `\f`, `\n`, `\r`, `\t`, otherwise
///   `\u00xx`) and leave every other character, including U+007F and U+2028, as is.
pub fn js_stringify(value: &Value) -> String {
    let mut out = String::new();
    js_write(value, &mut out);
    out
}

/// The value `JSON.parse(JSON.stringify(value))` yields, so a later `serde_json` encoding of it matches
/// JavaScript for index-like keys and whole-valued numbers.
pub(crate) fn js_normalize(value: &Value) -> Value {
    match value {
        Value::Number(number) => {
            let text = js_number(number);
            serde_json::from_str::<Number>(&text).map_or_else(|_| value.clone(), Value::Number)
        }
        Value::Array(items) => Value::Array(items.iter().map(js_normalize).collect()),
        Value::Object(map) => Value::Object(
            js_key_order(map)
                .into_iter()
                .map(|(key, item)| (key.clone(), js_normalize(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Python `repr(float)`.
fn python_float(number: &Number) -> String {
    let Some(float) = number.as_f64() else {
        return number.to_string();
    };
    let text = format!("{float:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..16).contains(&exponent) || float == 0.0 {
        let fixed = float.to_string();
        return if fixed.contains('.') {
            fixed
        } else {
            format!("{fixed}.0")
        };
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

fn python_string(text: &str, out: &mut String) {
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
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn python_write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) if number.is_f64() => out.push_str(&python_float(number)),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => python_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                python_write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (position, (key, item)) in map.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                python_string(key, out);
                out.push_str(": ");
                python_write(item, out);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(value)` with its defaults.
pub(crate) fn python_dumps(value: &Value) -> String {
    let mut out = String::new();
    python_write(value, &mut out);
    out
}

#[cfg(test)]
mod tests;
