//! The JavaScript text rules the bridge client reads a service reply by: `Number(string)`, `String.prototype.trim`
//! and the check that keeps a service message out of an error unless it is one plain line.

use std::sync::LazyLock;

use regex::Regex;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

use crate::bridge_unicode::{is_forbidden, is_mark, is_punctuation};
use crate::session_prompt::{js_space, js_trim};

mod constants;

use constants::DETAIL_LIMIT;

static ESCAPE_CODES: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]|\x1b[\]PX^_][^\x1b\x07]*(?:\x07|\x1b\\)").ok()
});
static PARTIAL_ESCAPE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\x1b(?:[\]PX^_][^\x1b\x07]*\x1b?|\[[\x30-\x3f]*[\x20-\x2f]*)$").ok());
static WHOLE_CSI: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]").ok());
static WHOLE_STRING_ESCAPE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\x1b[\]PX^_][^\x1b\x07]*(?:\x07|\x1b\\)").ok());
static DECIMAL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").ok());

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

/// `parseInt(text, 10)`: the leading run of decimal digits after white space and an optional sign, `None` for `NaN`.
pub(crate) fn js_parse_int(text: &str) -> Option<f64> {
    let text = text.trim_start_matches(js_space);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let value: f64 = digits.get(..end).filter(|run| !run.is_empty())?.parse().ok()?;
    Some(if negative { -value } else { value })
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
    let cleaned = compose(&collapse_spaces(message));
    (cleaned.encode_utf16().count() <= DETAIL_LIMIT && cleaned == message.replace('`', "'")).then(|| message.to_owned())
}

/// The tail of the label cleaner: composed (NFC), backticks as apostrophes, no combining marks after the start, white
/// space or punctuation.
fn compose(text: &str) -> String {
    let mapped: String = text
        .nfc()
        .map(|c| match c {
            '`' | '\u{ff40}' | '\u{2cb}' | '\u{1fef}' | '\u{2035}' => '\'',
            other => other,
        })
        .collect();
    strip_leading_marks(&mapped)
}

/// `rc(text, limit)`: a name as the upstream label cleaner shows it. Cut to `limit * 8` UTF-16 units (an escape code
/// the cut splits goes whole when the full text finishes it), stripped of escape codes (at most four passes, so a
/// code the stripping rebuilds can survive), each run of control, format, line and paragraph separator characters a
/// single space, runs of spaces one space, trimmed, composed, and finally cut to `limit` units with an ellipsis.
pub(crate) fn clean_label(text: &str, limit: usize) -> String {
    let cut = cut_before_partial_escape(text, limit * 8);
    let spaced = collapse_spaces(&mask_controls(&strip_escape_codes(cut)));
    let composed = compose(&spaced);
    if composed.encode_utf16().count() > limit {
        format!("{}\u{2026}", prefix_within_units(&composed, limit))
    } else {
        composed
    }
}

/// `LX`: `text` cut to `limit` units, less an escape code that the cut leaves unfinished and the text finishes.
fn cut_before_partial_escape(text: &str, limit: usize) -> &str {
    let cut = prefix_within_units(text, limit);
    if cut.len() == text.len() {
        return cut;
    }
    let finishes = |tail: &str| {
        let pattern = if tail.chars().nth(1) == Some('[') {
            &WHOLE_CSI
        } else {
            &WHOLE_STRING_ESCAPE
        };
        pattern.as_ref().is_some_and(|pattern| pattern.is_match(tail))
    };
    PARTIAL_ESCAPE
        .as_ref()
        .and_then(|partial| partial.find(cut))
        .map(|found| found.start())
        .filter(|start| text.get(*start..).is_some_and(finishes))
        .map_or(cut, |start| cut.get(..start).unwrap_or(cut))
}

/// `kX`: up to four passes that remove the escape codes.
fn strip_escape_codes(text: &str) -> String {
    let mut current = text.to_owned();
    for _ in 0..4 {
        let next = ESCAPE_CODES.as_ref().map_or_else(
            || current.clone(),
            |pattern| pattern.replace_all(&current, "").into_owned(),
        );
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// `wk`: each run of control, format, line and paragraph separator characters becomes one space.
fn mask_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut masking = false;
    for c in text.chars() {
        let masked = matches!(
            get_general_category(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        );
        if !masked {
            out.push(c);
        } else if !masking {
            out.push(' ');
        }
        masking = masked;
    }
    out
}

/// `Kt(text)`: the text of a control reply error as the upstream cleaner leaves it: cut to 4,096 UTF-16 units (a pair
/// the cut would split goes whole), stripped of control, format, line and paragraph separator, default-ignorable and
/// braille blank characters and of every space separator but the plain space, then cut to 1,024 units the same way.
/// Unpaired surrogates, which the upstream cleaner also removes, cannot be held in a Rust string.
pub(crate) fn clean_reply_text(text: &str) -> String {
    let kept: String = prefix_within_units(text, 4096)
        .chars()
        .filter(|c| !is_stripped(*c))
        .collect();
    prefix_within_units(&kept, DETAIL_LIMIT).to_owned()
}

fn prefix_within_units(text: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        units += c.len_utf16();
        if units > limit {
            return text.get(..index).unwrap_or(text);
        }
    }
    text
}

fn is_stripped(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    ) || (c != ' ' && get_general_category(c) == GeneralCategory::SpaceSeparator)
        || c == '\u{2800}'
        || is_default_ignorable(c)
}

/// The `Default_Ignorable_Code_Point` property.
fn is_default_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fff8}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
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
        if is_mark(c) && (removing || previous.is_none_or(|before| js_space(before) || is_punctuation(before))) {
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
