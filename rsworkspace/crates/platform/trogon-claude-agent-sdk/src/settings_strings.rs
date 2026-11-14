//! Lone surrogates in parsed settings documents.
//!
//! `JSON.parse` and `json.loads` return a string holding a lone surrogate for a `"\ud800"` escape, and Node's
//! UTF-16LE decoding keeps one, which a `serde_json` string cannot hold. A resolve keeps such strings on its thread.
//! Before it reads any document it reserves every private-use code point an input of the resolve may hold: the
//! settings texts, escapes included, the managed policy documents, the settings paths and the working directory. In
//! the settings value each lone surrogate then stands as a private-use code point no input holds, the same one
//! wherever it appears, so the engine counts it as many UTF-16 units as JavaScript does and matches it as one code
//! point, and no string the resolve returns can hold a stand-in it did not make. When the resolve ends, every
//! stand-in turns back into its lone surrogate, or into U+FFFD.

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    mem,
};

use serde_json::Value;

use crate::{
    constants::{SETTINGS_SURROGATE_STAND_IN_RANGES, SETTINGS_SURROGATE_STAND_IN_STARTS, SURROGATE_FIRST},
    js_array_mark::ArrayMark,
    python_code_point::PythonCodePoint,
    python_json_value::PythonJsonValue,
    python_string::PythonString,
};

/// What a resolve keeps: the private-use code points its inputs hold, the stand-in of each
/// lone surrogate with the surrogate it stands for, and the mark of its merged arrays.
#[derive(Default)]
struct KeptStrings {
    reserved: HashSet<char>,
    stand_ins: HashMap<u32, char>,
    surrogates: HashMap<char, u32>,
    mark: Option<ArrayMark>,
}

thread_local! {
    static PYTHON_STRINGS: RefCell<Option<KeptStrings>> = const { RefCell::new(None) };
}

/// Keeps the strings with lone surrogates of a resolve on this thread until it is dropped.
pub(crate) struct SurrogateStandIns {
    previous: Option<KeptStrings>,
}

impl SurrogateStandIns {
    pub(crate) fn start() -> Self {
        Self {
            previous: PYTHON_STRINGS.replace(Some(KeptStrings::default())),
        }
    }
}

impl Drop for SurrogateStandIns {
    fn drop(&mut self) {
        PYTHON_STRINGS.set(self.previous.take());
    }
}

fn private_use(c: char) -> bool {
    let value = u32::from(c);
    SETTINGS_SURROGATE_STAND_IN_RANGES
        .iter()
        .any(|&(first, last)| (first..=last).contains(&value))
}

fn with_kept<T>(work: impl FnOnce(&mut KeptStrings) -> T) -> Option<T> {
    PYTHON_STRINGS.with_borrow_mut(|kept| kept.as_mut().map(work))
}

/// Reserves every private-use code point of `text`, and of each `\uXXXX` escape in it, so no lone surrogate stands
/// as one. A string JSON text spells holds no code point outside that set.
pub(crate) fn reserve_text(text: &str) {
    with_kept(|kept| {
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            kept.reserve(c);
            if c != '\\' {
                continue;
            }
            match chars.next() {
                Some('u') => {
                    let digits: String = chars.clone().take(4).collect();
                    if let Some(escaped) = u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                        kept.reserve(escaped);
                    }
                }
                Some(other) => kept.reserve(other),
                None => {}
            }
        }
    });
}

/// Reserves every private-use code point of the strings and keys of `value`.
pub(crate) fn reserve_value(value: &Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) => reserve_text(text),
            Value::Array(items) => pending.extend(items),
            Value::Object(entries) => entries.iter().for_each(|(key, entry)| {
                reserve_text(key);
                pending.push(entry);
            }),
            _ => {}
        }
    }
}

impl KeptStrings {
    fn reserve(&mut self, c: char) {
        if private_use(c) {
            self.reserved.insert(c);
        }
    }

    fn free(&self, c: char) -> bool {
        !self.reserved.contains(&c) && !self.surrogates.contains_key(&c) && self.mark != Some(ArrayMark::new(c))
    }

    /// The stand-in of a lone surrogate: its place in the first range of [`SETTINGS_SURROGATE_STAND_IN_STARTS`]
    /// that is free there, else the first free private-use code point. `None` when every one is taken.
    fn stand_in(&mut self, surrogate: u32) -> Option<char> {
        if let Some(&stand_in) = self.stand_ins.get(&surrogate) {
            return Some(stand_in);
        }
        let offset = surrogate.checked_sub(SURROGATE_FIRST)?;
        let preferred = SETTINGS_SURROGATE_STAND_IN_STARTS
            .iter()
            .filter_map(|&start| start.checked_add(offset).and_then(char::from_u32));
        let any = SETTINGS_SURROGATE_STAND_IN_RANGES
            .iter()
            .flat_map(|&(first, last)| (first..=last).filter_map(char::from_u32));
        let stand_in = preferred.chain(any).find(|&c| self.free(c))?;
        self.stand_ins.insert(surrogate, stand_in);
        self.surrogates.insert(stand_in, surrogate);
        Some(stand_in)
    }
}

/// The length JavaScript gives a settings string, in UTF-16 units. Each stand-in the resolve on this thread made
/// counts as the one unit of the lone surrogate it stands for, though a stand-in past the Basic Multilingual Plane,
/// which a resolve makes only once its inputs hold every private-use code point of that plane, takes two in Rust.
pub(crate) fn js_length(text: &str) -> usize {
    let units = text.encode_utf16().count();
    let wide_private_use = |c: &char| c.len_utf16() == 2 && private_use(*c);
    if !text.chars().any(|c| wide_private_use(&c)) {
        return units;
    }
    PYTHON_STRINGS.with_borrow(|kept| {
        let wide_stand_ins = kept.as_ref().map_or(0, |kept| {
            text.chars()
                .filter(wide_private_use)
                .filter(|c| kept.surrogates.contains_key(c))
                .count()
        });
        units.saturating_sub(wide_stand_ins)
    })
}

/// The mark of the arrays the TypeScript-dialect merges of the resolve on this thread write: the last private-use
/// code point no input holds and no lone surrogate stands as when the first merge asks, the same for the rest of the
/// resolve, and never a stand-in after it. It is taken from the end so the stand-ins keep their places in the Basic
/// Multilingual Plane. [`ArrayMark::FALLBACK`] outside a resolve that keeps strings, or when every private-use code
/// point is taken, where an input holding it would merge as a JavaScript array never does.
pub(crate) fn array_mark() -> ArrayMark {
    with_kept(|kept| {
        if let Some(mark) = kept.mark {
            return mark;
        }
        let mark = SETTINGS_SURROGATE_STAND_IN_RANGES
            .iter()
            .rev()
            .flat_map(|&(first, last)| (first..=last).rev().filter_map(char::from_u32))
            .find(|&c| kept.free(c))
            .map_or(ArrayMark::FALLBACK, ArrayMark::new);
        kept.mark = Some(mark);
        mark
    })
    .unwrap_or(ArrayMark::FALLBACK)
}

/// The `\uXXXX` escape `text` starts with, as the UTF-16 unit it spells.
fn escaped_unit(text: &str) -> Option<u16> {
    let digits = text.strip_prefix("\\u")?.get(..4)?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

/// `text` with each `\uXXXX` escape of a lone surrogate spelled as the escape of its stand-in while a resolve keeps
/// strings, so `serde_json` reads the string `JSON.parse` reads. An escaped high surrogate followed by an escaped
/// low one is a pair, as `JSON.parse` reads it, and stays. Outside a string every backslash makes the text invalid,
/// with or without the rewrite, so escapes are found without tracking strings.
pub(crate) fn stand_in_escapes(text: &str) -> Cow<'_, str> {
    if !text.contains("\\u") {
        return Cow::Borrowed(text);
    }
    with_kept(|kept| {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        let mut changed = false;
        while let Some(at) = rest.find('\\') {
            let (head, tail) = rest.split_at(at);
            out.push_str(head);
            let Some(unit) = escaped_unit(tail).filter(|&unit| (0xD800..=0xDFFF).contains(&unit)) else {
                let length = tail.chars().take(2).map(char::len_utf8).sum();
                let (escape, after) = tail.split_at(length);
                out.push_str(escape);
                rest = after;
                continue;
            };
            let (escape, after) = tail.split_at(6);
            let paired = (0xD800..=0xDBFF).contains(&unit)
                && escaped_unit(after).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low));
            if paired {
                let (low, after) = after.split_at(6);
                out.push_str(escape);
                out.push_str(low);
                rest = after;
                continue;
            }
            match kept.stand_in(u32::from(unit)) {
                Some(stand_in) => {
                    for half in stand_in.encode_utf16(&mut [0; 2]) {
                        let _ = write!(out, "\\u{half:04x}");
                    }
                    changed = true;
                }
                None => out.push_str(escape),
            }
            rest = after;
        }
        out.push_str(rest);
        if changed { Cow::Owned(out) } else { Cow::Borrowed(text) }
    })
    .unwrap_or(Cow::Borrowed(text))
}

/// Reserves the private-use code points of the strings and keys of a document `json.loads` read. `false` when one
/// already stands for a lone surrogate, which a resolve that reserved its inputs first never sees.
pub(crate) fn take_strings(value: &PythonJsonValue) -> bool {
    with_kept(|kept| {
        let mut pending = vec![value];
        let mut clear = true;
        let mut take = |text: &PythonString, kept: &mut KeptStrings| {
            for c in text.code_points().filter_map(PythonCodePoint::as_char) {
                clear &= !kept.surrogates.contains_key(&c);
                kept.reserve(c);
            }
        };
        while let Some(value) = pending.pop() {
            match value {
                PythonJsonValue::String(text) => take(text, kept),
                PythonJsonValue::Array(items) => pending.extend(items),
                PythonJsonValue::Object(entries) => entries.iter().for_each(|(key, entry)| {
                    take(key, kept);
                    pending.push(entry);
                }),
                _ => {}
            }
        }
        clear
    })
    .unwrap_or(true)
}

/// The settings string for a `str` that `json.loads` read: the text itself or, for one holding a lone surrogate,
/// the text with each lone surrogate as its stand-in while a Python-dialect resolve keeps strings. `None` for a
/// lone surrogate outside such a resolve, or when every private-use code point is taken.
pub(crate) fn python_string(text: PythonString) -> Option<String> {
    let text = match text.into_string() {
        Ok(text) => return Some(text),
        Err(text) => text,
    };
    with_kept(|kept| {
        text.code_points()
            .map(|code_point| code_point.as_char().or_else(|| kept.stand_in(code_point.as_u32())))
            .collect::<Option<String>>()
    })
    .flatten()
}

/// The lone surrogate each stand-in of `text` stands for, while the resolve that made it keeps strings.
fn kept_string(text: &str) -> Option<PythonString> {
    if !text.chars().any(private_use) {
        return None;
    }
    PYTHON_STRINGS.with_borrow(|kept| {
        let kept = kept.as_ref()?;
        if !text.chars().any(|c| kept.surrogates.contains_key(&c)) {
            return None;
        }
        let mut exact = PythonString::new();
        for c in text.chars() {
            exact.push(match kept.surrogates.get(&c) {
                Some(&surrogate) => u16::try_from(surrogate).map_or(PythonCodePoint::from(c), PythonCodePoint::from),
                None => PythonCodePoint::from(c),
            });
        }
        Some(exact)
    })
}

/// `text` with each stand-in the resolve on this thread made spelled as the `\uXXXX` escape of its lone surrogate,
/// as `JSON.stringify` writes a lone surrogate. No input of the resolve holds a stand-in, so in JSON text the
/// resolve printed every one is inside a string or key.
pub(crate) fn escape_stand_ins(text: String) -> String {
    PYTHON_STRINGS.with_borrow(|kept| {
        let Some(kept) = kept.as_ref().filter(|kept| !kept.surrogates.is_empty()) else {
            return text;
        };
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match kept.surrogates.get(&c) {
                Some(surrogate) => {
                    let _ = write!(out, "\\u{surrogate:04x}");
                }
                None => out.push(c),
            }
        }
        out
    })
}

/// The exact `str` a settings string stands for.
pub(crate) fn exact_string(text: &str) -> PythonString {
    kept_string(text).unwrap_or_else(|| text.into())
}

/// Turns each stand-in of a settings string into U+FFFD.
fn approximate(text: &mut String) {
    if let Some(kept) = kept_string(text) {
        *text = kept
            .code_points()
            .map(|code_point| code_point.as_char().unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
    }
}

/// Replaces every stand-in of `value`, keys included, with U+FFFD. Of keys that become equal, the last entry
/// stays, in the place of the first.
pub(crate) fn approximate_strings(value: &mut Value) {
    match value {
        Value::String(text) => approximate(text),
        Value::Array(items) => items.iter_mut().for_each(approximate_strings),
        Value::Object(entries) => {
            if entries.keys().any(|key| kept_string(key).is_some()) {
                *entries = mem::take(entries)
                    .into_iter()
                    .map(|(mut key, entry)| {
                        approximate(&mut key);
                        (key, entry)
                    })
                    .collect();
            }
            entries.values_mut().for_each(approximate_strings);
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "settings_strings_tests.rs"]
mod tests;
