//! The two JSON writers settings travel through: TypeScript `JSON.stringify` and Python `json.dumps`.
//!
//! They differ from `serde_json` where it matters for byte-identical command lines: JavaScript lists
//! array-index keys first and prints a whole-valued float without a fraction, and Python separates with
//! `", "` and `": "` and escapes every character outside printable ASCII.

use serde_json::{Map, Number, Value};

/// JavaScript `Number::toString` for a finite number.
fn js_number(number: &Number) -> String {
    if number.is_i64() || number.is_u64() {
        let exact = number.as_i64().map(|n| n.unsigned_abs()).or_else(|| number.as_u64());
        if exact.is_some_and(|n| n <= (1_u64 << 53)) {
            return number.to_string();
        }
    }
    let Some(float) = number.as_f64() else {
        return number.to_string();
    };
    if float == 0.0 {
        return "0".to_owned();
    }
    let magnitude = float.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return float.to_string();
    }
    let text = format!("{float:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

/// Whether `key` is a canonical array index, which JavaScript orders first and numerically.
fn array_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn js_write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&js_number(number)),
        Value::String(text) => out.push_str(&Value::String(text.clone()).to_string()),
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
            let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
            let mut named: Vec<(&String, &Value)> = Vec::new();
            for (key, item) in map {
                match array_index(key) {
                    Some(index) => indexed.push((index, key, item)),
                    None => named.push((key, item)),
                }
            }
            indexed.sort_by_key(|(index, _, _)| *index);
            out.push('{');
            let entries = indexed.into_iter().map(|(_, key, item)| (key, item)).chain(named);
            for (position, (key, item)) in entries.enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                js_write(item, out);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(value)`.
pub(crate) fn js_stringify(value: &Value) -> String {
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
        Value::Object(map) => {
            let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
            let mut named: Vec<(&String, &Value)> = Vec::new();
            for (key, item) in map {
                match array_index(key) {
                    Some(index) => indexed.push((index, key, item)),
                    None => named.push((key, item)),
                }
            }
            indexed.sort_by_key(|(index, _, _)| *index);
            let mut out = Map::new();
            for (key, item) in indexed.into_iter().map(|(_, key, item)| (key, item)).chain(named) {
                out.insert(key.clone(), js_normalize(item));
            }
            Value::Object(out)
        }
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
