//! The JavaScript string and number semantics the browser SSE port depends on: the `\s` character class, `String(x)`
//! on a parsed JSON value, `parseInt`, UTF-16 truncation, and the control-character strip the transport applies to
//! text it logs or returns.

use serde_json::Value;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{constants::JS_MAX_EXACT_INTEGER, settings_json::js_number};

/// Whether a regular expression `\s` matches `ch` in JavaScript: the ECMAScript white space and line terminators.
pub(crate) fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `String(value)` threw: a JSON object with its own `toString` key cannot be turned into a primitive.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct JsConversionError;

/// `String(value)` for a value `JSON.parse` produced.
pub(crate) fn js_to_string(value: &Value) -> Result<String, JsConversionError> {
    match value {
        Value::Null => Ok("null".to_owned()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Number(number) => Ok(js_number(number)),
        Value::String(text) => Ok(text.clone()),
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(if item.is_null() {
                    String::new()
                } else {
                    js_to_string(item)?
                });
            }
            Ok(parts.join(","))
        }
        Value::Object(map) if map.contains_key("toString") => Err(JsConversionError),
        Value::Object(_) => Ok("[object Object]".to_owned()),
    }
}

/// `parseInt(text, 10)`: the leading digits after optional white space and a sign, or `None` for NaN.
fn js_parse_int(text: &str) -> Option<f64> {
    let trimmed = text.trim_start_matches(is_js_whitespace);
    let (negative, unsigned) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits = unsigned
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .unwrap_or_default();
    let magnitude = digits.parse::<f64>().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// `number` when it is a positive safe integer (below 2^53).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a positive integer below 2^53, which a double holds exactly"
)]
pub(crate) fn positive_safe_integer(number: f64) -> Option<u64> {
    (number > 0.0 && number < JS_MAX_EXACT_INTEGER as f64 && number.fract() == 0.0).then_some(number as u64)
}

/// The sequence number `Ob` reads from the text of an SSE `id` field.
pub(crate) fn sequence_number_text(text: &str) -> Option<u64> {
    js_parse_int(text).and_then(positive_safe_integer)
}

/// The sequence number `Ob` reads from a JSON value: a number as is, anything else as the digits `parseInt` finds in
/// `String(value ?? "")`. `None` stands for `undefined`.
pub(crate) fn sequence_number(value: Option<&Value>) -> Result<Option<u64>, JsConversionError> {
    Ok(match value {
        Some(Value::Number(number)) => number.as_f64().and_then(positive_safe_integer),
        None | Some(Value::Null) => None,
        Some(other) => sequence_number_text(&js_to_string(other)?),
    })
}

/// `text` cut to at most `limit` UTF-16 code units without splitting a surrogate pair.
pub(crate) fn truncate_utf16(text: &str, limit: usize) -> String {
    let mut units = 0;
    let mut kept = String::new();
    for ch in text.chars() {
        units += ch.len_utf16();
        if units > limit {
            break;
        }
        kept.push(ch);
    }
    kept
}

/// `text` without the control, format, line separator and paragraph separator characters (`\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}`).
pub(crate) fn strip_invisible(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !matches!(
                get_general_category(*ch),
                GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::Surrogate
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
            )
        })
        .collect()
}

/// `text` as the transport writes it into a log line: invisible characters removed, then cut to 64 UTF-16 units.
pub(crate) fn sanitize_log_text(text: &str) -> String {
    truncate_utf16(&strip_invisible(text), 64)
}

/// `JSON.parse`, tolerant of the lone surrogate escapes JavaScript strings can hold and Rust strings cannot.
///
/// Text `serde_json` accepts parses as is. Otherwise every unpaired `\uD800` to `\uDFFF` escape is replaced by
/// `replacement` (an escape such as `�`) and the text is parsed again.
pub(crate) fn parse_json(text: &str, replacement: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text).or_else(|error| {
        let replaced = replace_lone_surrogates(text, replacement);
        if replaced == text {
            Err(error)
        } else {
            serde_json::from_str(&replaced)
        }
    })
}

fn escape_unit(chars: &[char], at: usize) -> Option<u32> {
    let window = chars.get(at..at + 6)?;
    if window[0] != '\\' || window[1] != 'u' {
        return None;
    }
    window[2..]
        .iter()
        .try_fold(0, |unit, ch| Some(unit * 16 + ch.to_digit(16)?))
}

fn replace_lone_surrogates(text: &str, replacement: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let unit = escape_unit(&chars, at);
        match unit {
            Some(0xd800..=0xdbff) if matches!(escape_unit(&chars, at + 6), Some(0xdc00..=0xdfff)) => {
                out.extend(&chars[at..at + 12]);
                at += 12;
            }
            Some(0xd800..=0xdfff) => {
                out.push_str(replacement);
                at += 6;
            }
            _ if chars[at] == '\\' => {
                out.extend(&chars[at..(at + 2).min(chars.len())]);
                at += 2;
            }
            _ => {
                out.push(chars[at]);
                at += 1;
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "js_text_tests.rs"]
mod tests;
