//! The object the pinned TypeScript `resolveSettings` returns, with every string exact. A JavaScript string holds
//! any UTF-16 units, so a lone surrogate a settings file spells stays in it, while a Rust `String` cannot hold one.
//! [`PythonJsonValue`] holds such strings as [`PythonString`]s, with each lone surrogate as its code point, and here
//! each number as `JSON.stringify` spells the double it stands for, so the value is the one a `JSON.parse` that
//! keeps lone surrogates reads back from the text [`resolve_settings_json_with_state`] prints.
//!
//! [`resolve_settings_json_with_state`]: crate::resolve_settings_json_with_state

use std::{fmt::Write as _, str::FromStr};

use serde_json::{Number, Value};

use crate::{
    constants::JS_MAX_EXACT_INTEGER,
    js_object_order::{array_index, js_key_order},
    python_json_value::{ExactInteger, PythonJsonValue, PythonNumber},
    python_string::PythonString,
    settings_json::{js_float, js_number},
    settings_strings::exact_string,
};

/// The number `JSON.stringify` spells as `number` prints: an integer when the spelling holds only digits, else a
/// float.
fn js_exact_number(number: &Number) -> PythonNumber {
    match ExactInteger::from_str(&js_number(number)) {
        Ok(integer) => PythonNumber::Int(integer),
        Err(_) => PythonNumber::Float(number.as_f64().unwrap_or(f64::NAN)),
    }
}

/// The value a TypeScript-dialect resolve on this thread holds, with every stand-in back to its lone surrogate and
/// every object in JavaScript enumeration order. Infinities must already be `null`.
pub(crate) fn js_exact_value(value: &Value) -> PythonJsonValue {
    match value {
        Value::Null => PythonJsonValue::Null,
        Value::Bool(flag) => PythonJsonValue::Bool(*flag),
        Value::Number(number) => PythonJsonValue::Number(js_exact_number(number)),
        Value::String(text) => PythonJsonValue::String(exact_string(text)),
        Value::Array(items) => PythonJsonValue::Array(items.iter().map(js_exact_value).collect()),
        Value::Object(entries) => PythonJsonValue::Object(
            js_key_order(entries)
                .into_iter()
                .map(|(key, entry)| (exact_string(key), js_exact_value(entry)))
                .collect(),
        ),
    }
}

/// `JSON.stringify` of the JavaScript value `value` stands for. Each lone surrogate prints as its `\udxxx`
/// escape, `"`, `\` and the control characters below U+0020 print escaped as `JSON.stringify` escapes them, and
/// every other character prints as is. An integer prints as the double it rounds to, and a float as
/// `Number.prototype.toString` prints it, `null` when it is not finite. Object keys print in JavaScript
/// enumeration order: array indices ascending first, then the other keys in order. It writes with a loop, so a
/// value of any depth prints without exhausting the stack.
pub fn js_stringify_exact(value: &PythonJsonValue) -> String {
    let mut out = String::new();
    let mut pending = vec![Pending::Value(value)];
    while let Some(next) = pending.pop() {
        match next {
            Pending::Text(text) => out.push_str(text),
            Pending::Key(key) => {
                write_string(key, &mut out);
                out.push(':');
            }
            Pending::Value(value) => write_value(value, &mut out, &mut pending),
        }
    }
    out
}

/// What [`js_stringify_exact`] still has to write, last first.
enum Pending<'a> {
    Value(&'a PythonJsonValue),
    Key(&'a PythonString),
    Text(&'static str),
}

fn write_number(number: &PythonNumber, out: &mut String) {
    match number {
        PythonNumber::Int(integer) => {
            let exact = integer
                .to_i64()
                .is_some_and(|value| value.unsigned_abs() <= JS_MAX_EXACT_INTEGER);
            if exact {
                out.push_str(integer.as_str());
            } else {
                out.push_str(&js_float(integer.to_f64()));
            }
        }
        PythonNumber::Float(float) => out.push_str(&js_float(*float)),
    }
}

fn write_string(text: &PythonString, out: &mut String) {
    out.push('"');
    for code_point in text.code_points() {
        match code_point.as_char() {
            Some('"') => out.push_str("\\\""),
            Some('\\') => out.push_str("\\\\"),
            Some('\u{8}') => out.push_str("\\b"),
            Some('\u{c}') => out.push_str("\\f"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some('\t') => out.push_str("\\t"),
            Some(c @ '\u{0}'..='\u{1f}') => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            Some(c) => out.push(c),
            None => {
                let _ = write!(out, "\\u{:04x}", code_point.as_u32());
            }
        }
    }
    out.push('"');
}

/// Writes a scalar, or the opening bracket of an array or object with the rest of it pushed onto `pending`.
fn write_value<'a>(value: &'a PythonJsonValue, out: &mut String, pending: &mut Vec<Pending<'a>>) {
    match value {
        PythonJsonValue::Null => out.push_str("null"),
        PythonJsonValue::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        PythonJsonValue::Number(number) => write_number(number, out),
        PythonJsonValue::String(text) => write_string(text, out),
        PythonJsonValue::Array(items) => {
            out.push('[');
            pending.push(Pending::Text("]"));
            for (position, item) in items.iter().enumerate().rev() {
                pending.push(Pending::Value(item));
                if position > 0 {
                    pending.push(Pending::Text(","));
                }
            }
        }
        PythonJsonValue::Object(entries) => {
            out.push('{');
            pending.push(Pending::Text("}"));
            let mut indexed = Vec::new();
            let mut named = Vec::new();
            for (key, entry) in entries {
                match key.as_str().and_then(array_index) {
                    Some(index) => indexed.push((index, key, entry)),
                    None => named.push((key, entry)),
                }
            }
            indexed.sort_by_key(|(index, _, _)| *index);
            let ordered: Vec<_> = indexed
                .into_iter()
                .map(|(_, key, entry)| (key, entry))
                .chain(named)
                .collect();
            for (position, (key, entry)) in ordered.into_iter().enumerate().rev() {
                pending.push(Pending::Value(entry));
                pending.push(Pending::Key(key));
                if position > 0 {
                    pending.push(Pending::Text(","));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "settings_exact_tests.rs"]
mod tests;
