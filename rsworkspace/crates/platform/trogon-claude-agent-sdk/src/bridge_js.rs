//! The JavaScript text rules the bridge client reads a service reply by: `Number(string)`, `String.prototype.trim`
//! and the check that keeps a service message out of an error unless it is one plain line.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::bridge_unicode::{is_forbidden, is_mark, is_punctuation, is_space};

const DETAIL_LIMIT: usize = 1024;

static DECIMAL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").ok());

/// A character the JavaScript `\s` class and `trim` treat as white space.
pub(crate) fn is_js_space(c: char) -> bool {
    is_space(c)
}

/// `text.trim()`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `Number(text)`: `NaN` for text that is not a number.
pub(crate) fn js_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    for (prefixes, radix) in [(["0x", "0X"], 16), (["0o", "0O"], 8), (["0b", "0B"], 2)] {
        if let Some(digits) = prefixes.iter().find_map(|prefix| text.strip_prefix(prefix)) {
            return radix_number(digits, radix);
        }
    }
    match text {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if DECIMAL.as_ref().is_some_and(|decimal| decimal.is_match(text)) => text.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

fn radix_number(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    digits
        .chars()
        .try_fold(0.0, |value, digit| {
            digit
                .to_digit(radix)
                .map(|digit| value * f64::from(radix) + f64::from(digit))
        })
        .unwrap_or(f64::NAN)
}

/// `Number.isSafeInteger(value)` as the integer it names.
pub(crate) fn safe_integer(value: f64) -> Option<i64> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is a whole number within the safe range"
    )]
    (value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE).then_some(value as i64)
}

/// The service's own text when it is one plain line, otherwise `None`, as the upstream error detail check decides:
/// at most 1,024 UTF-16 units, no control, invisible or unassigned character and no white space but a single ASCII
/// space, and nothing the upstream text cleaning would change apart from a backtick becoming an apostrophe.
pub(crate) fn plain_detail(message: &str) -> Option<String> {
    if message.encode_utf16().count() > DETAIL_LIMIT || message.chars().any(is_forbidden) {
        return None;
    }
    let mapped: String = collapse_spaces(message)
        .nfc()
        .map(|c| match c {
            '`' | '\u{ff40}' | '\u{2cb}' | '\u{1fef}' | '\u{2035}' => '\'',
            other => other,
        })
        .collect();
    let cleaned = strip_leading_marks(&mapped);
    (cleaned.encode_utf16().count() <= DETAIL_LIMIT && cleaned == message.replace('`', "'")).then(|| message.to_owned())
}

fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c != ' ' || !out.ends_with(' ') {
            out.push(c);
        }
    }
    js_trim(&out).to_owned()
}

/// Removes each run of combining marks that follows the start, white space or punctuation.
fn strip_leading_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous: Option<char> = None;
    let mut removing = false;
    for c in text.chars() {
        if is_mark(c) && (removing || previous.is_none_or(|before| is_js_space(before) || is_punctuation(before))) {
            removing = true;
        } else {
            removing = false;
            out.push(c);
        }
        previous = Some(c);
    }
    out
}

#[cfg(test)]
#[path = "bridge_js_tests.rs"]
mod tests;
