//! CPython 3.12's `json.loads`: the C scanner (`Modules/_json.c`) with `strict=True`, so no control character
//! inside a string and only `" \t\n\r"` as whitespace, reading `NaN`, `Infinity` and `-Infinity` and keeping every
//! integer exact. It reads code points, not bytes, so a string keeps a lone surrogate.
//!
//! The scanner nests without recursing, so a document nested thousands of levels deep cannot exhaust the stack,
//! and raises `RecursionError` past a depth its caller gives: Python's own limit depends on how deep the stack
//! already is where `json.loads` runs.

use indexmap::IndexMap;

use crate::python_c_stack::PythonCStack;
use crate::{
    python_code_point::PythonCodePoint,
    python_json_error::{JsonContainer, JsonDecodeError, JsonDecodeErrorKind, PythonJsonError},
    python_json_value::{PythonJsonValue, PythonNumber},
    python_string::PythonString,
};

/// Python `json.loads(text)` called from a plain function or module-level code.
///
/// It raises what Python raises, with the same message. Python raises `RecursionError` on entering the array or
/// object that would open more C recursion checks than CPython 3.12 allows; called from a plain function, that is
/// the 9998th level. Inside a coroutine the C layers that drive it hold checks too, and the limit is lower.
pub fn python_json_loads(text: &str) -> Result<PythonJsonValue, PythonJsonError> {
    loads_text(text, PythonCStack::MODULE.json_depth())
}

/// `json.loads(text)` for a `str`, raising `RecursionError` on entering a container past `max_depth` open ones.
pub(crate) fn loads_text(text: &str, max_depth: usize) -> Result<PythonJsonValue, PythonJsonError> {
    let text: Vec<PythonCodePoint> = text.chars().map(PythonCodePoint::from).collect();
    if text.first() == Some(&PythonCodePoint::from('\u{feff}')) {
        let error = JsonDecodeError::new(JsonDecodeErrorKind::ByteOrderMark, &text, 0);
        return Err(PythonJsonError::Decode(error));
    }
    parse(&text, max_depth)
}

/// `json.loads(data)` for bytes, as `json.load(response)` reads a remote schema. `None` when Python raises. A lone
/// surrogate stays in the string or key that holds it, as Python keeps it.
pub(crate) fn loads(data: &[u8], max_depth: usize) -> Option<PythonJsonValue> {
    parse(&decode(data)?, max_depth).ok()
}

/// Drops a value the scanner built. [`PythonJsonValue`] drops without recursing, so this is a plain drop
/// that names where a value as deep as the scanner allows ends.
pub(crate) fn drop_python_deep(value: PythonJsonValue) {
    drop(value);
}

/// `JSONDecoder.decode` after the `str` check: whitespace, one value, whitespace, then nothing.
fn parse(text: &[PythonCodePoint], max_depth: usize) -> Result<PythonJsonValue, PythonJsonError> {
    let scanner = Scanner { text, max_depth };
    let mut frames = Vec::new();
    let scanned = scanner.scan(scanner.whitespace(0), &mut frames);
    for frame in frames {
        drop_python_deep(frame.into_value());
    }
    let (value, end) = scanned?;
    let end = scanner.whitespace(end);
    if end != text.len() {
        drop_python_deep(value);
        return Err(scanner.error(JsonDecodeErrorKind::ExtraData, end));
    }
    Ok(value)
}

/// `data.decode(json.detect_encoding(data), "surrogatepass")`, or `None` for `UnicodeDecodeError`.
fn decode(data: &[u8]) -> Option<Vec<PythonCodePoint>> {
    match data {
        [0xFF, 0xFE, 0, 0, rest @ ..] => utf32(rest, false),
        [0, 0, 0xFE, 0xFF, rest @ ..] => utf32(rest, true),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => utf8(rest),
        [0, second, _, _, ..] => {
            if *second == 0 {
                utf32(data, true)
            } else {
                utf16(data, true)
            }
        }
        [_, 0, third, fourth, ..] => {
            if *third == 0 && *fourth == 0 {
                utf32(data, false)
            } else {
                utf16(data, false)
            }
        }
        [0, _] => utf16(data, true),
        [_, 0] => utf16(data, false),
        _ => utf8(data),
    }
}

/// The `utf-8` codec with `surrogatepass`: an encoded surrogate decodes to itself, anything else invalid raises.
fn utf8(mut data: &[u8]) -> Option<Vec<PythonCodePoint>> {
    let mut out = Vec::with_capacity(data.len());
    loop {
        let (valid, rest) = match std::str::from_utf8(data) {
            Ok(text) => {
                out.extend(text.chars().map(PythonCodePoint::from));
                return Some(out);
            }
            Err(error) => data.split_at(error.valid_up_to()),
        };
        out.extend(String::from_utf8_lossy(valid).chars().map(PythonCodePoint::from));
        let [0xED, high @ 0xA0..=0xBF, low @ 0x80..=0xBF, tail @ ..] = rest else {
            return None;
        };
        out.push(PythonCodePoint::from(
            0xD000 | (u16::from(high & 0x3F) << 6) | u16::from(low & 0x3F),
        ));
        data = tail;
    }
}

/// The `utf-16` codecs with `surrogatepass`: a pair decodes to its code point and a lone surrogate to itself.
fn utf16(data: &[u8], big_endian: bool) -> Option<Vec<PythonCodePoint>> {
    let unit = |at: usize| {
        let bytes: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        let unit = if big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        };
        Some(PythonCodePoint::from(unit))
    };
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut at = 0;
    while at < data.len() {
        let first = unit(at)?;
        at += 2;
        if first.is_high_surrogate()
            && let Some(second) = unit(at)
            && second.is_low_surrogate()
        {
            out.push(PythonCodePoint::from_surrogates(first, second));
            at += 2;
            continue;
        }
        out.push(first);
    }
    Some(out)
}

/// The `utf-32` codecs with `surrogatepass`: anything past U+10FFFF or a truncated unit raises.
fn utf32(data: &[u8], big_endian: bool) -> Option<Vec<PythonCodePoint>> {
    let units = data.chunks_exact(4);
    if !units.remainder().is_empty() {
        return None;
    }
    units
        .map(|unit| {
            let bytes: [u8; 4] = unit.try_into().ok()?;
            let value = if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            };
            PythonCodePoint::try_from(value).ok()
        })
        .collect()
}

/// An object or array the scanner is inside of, with what it read so far.
enum Frame {
    Array(Vec<PythonJsonValue>),
    /// The members so far and the key of the member being read.
    Object(IndexMap<PythonString, PythonJsonValue>, PythonString),
}

impl Frame {
    /// Adds the item or member value just read, and returns the bracket that closes the container. A repeated
    /// key keeps its first position and takes the new value, as a `dict` does.
    fn attach(&mut self, value: PythonJsonValue) -> char {
        match self {
            Self::Array(items) => {
                items.push(value);
                ']'
            }
            Self::Object(members, key) => {
                if let Some(replaced) = members.insert(std::mem::take(key), value) {
                    drop_python_deep(replaced);
                }
                '}'
            }
        }
    }

    fn into_value(self) -> PythonJsonValue {
        match self {
            Self::Array(items) => PythonJsonValue::Array(items),
            Self::Object(members, _) => PythonJsonValue::Object(members),
        }
    }
}

/// `scan_once_unicode` and the functions it calls, over decoded text.
struct Scanner<'t> {
    text: &'t [PythonCodePoint],
    max_depth: usize,
}

impl Scanner<'_> {
    /// Whether the code point at `index` is `ascii`.
    fn is(&self, index: usize, ascii: char) -> bool {
        self.text.get(index) == Some(&PythonCodePoint::from(ascii))
    }

    fn is_digit(&self, index: usize) -> bool {
        self.text
            .get(index)
            .and_then(|c| c.as_char())
            .is_some_and(|c| c.is_ascii_digit())
    }

    /// The index past the digits from `index`.
    fn digits(&self, mut index: usize) -> usize {
        while self.is_digit(index) {
            index += 1;
        }
        index
    }

    /// The index past the whitespace from `index`.
    fn whitespace(&self, mut index: usize) -> usize {
        while [' ', '\t', '\n', '\r'].iter().any(|c| self.is(index, *c)) {
            index += 1;
        }
        index
    }

    fn starts_with(&self, index: usize, word: &str) -> bool {
        word.chars().enumerate().all(|(offset, c)| self.is(index + offset, c))
    }

    fn error(&self, kind: JsonDecodeErrorKind, pos: usize) -> PythonJsonError {
        PythonJsonError::Decode(JsonDecodeError::new(kind, self.text, pos))
    }

    /// The value at `index` and the index past it. Each object and array being read is a frame in `frames`
    /// instead of a recursive call, and the frames still open when it fails are left there for the caller.
    fn scan(&self, mut index: usize, frames: &mut Vec<Frame>) -> Result<(PythonJsonValue, usize), PythonJsonError> {
        'value: loop {
            let (mut value, mut next) = if self.is(index, '{') || self.is(index, '[') {
                if frames.len() >= self.max_depth {
                    let container = if self.is(index, '{') {
                        JsonContainer::Object
                    } else {
                        JsonContainer::Array
                    };
                    return Err(PythonJsonError::Recursion(container));
                }
                let at = self.whitespace(index + 1);
                if self.is(index, '[') {
                    if !self.is(at, ']') {
                        frames.push(Frame::Array(Vec::new()));
                        index = at;
                        continue 'value;
                    }
                    (PythonJsonValue::Array(Vec::new()), at + 1)
                } else {
                    if !self.is(at, '}') {
                        let (key, at) = self.key(at)?;
                        frames.push(Frame::Object(IndexMap::new(), key));
                        index = at;
                        continue 'value;
                    }
                    (PythonJsonValue::Object(IndexMap::new()), at + 1)
                }
            } else if self.is(index, '"') {
                let (text, next) = self.string(index + 1)?;
                (PythonJsonValue::String(text), next)
            } else {
                self.scalar(index)?
            };
            loop {
                let Some(frame) = frames.last_mut() else {
                    return Ok((value, next));
                };
                let at = self.whitespace(next);
                let close = frame.attach(value);
                if self.is(at, ',') {
                    index = self.whitespace(at + 1);
                    if let Frame::Object(_, key) = frame {
                        let (name, after) = self.key(index)?;
                        *key = name;
                        index = after;
                    }
                    continue 'value;
                }
                if !self.is(at, close) {
                    return Err(self.error(JsonDecodeErrorKind::ExpectingComma, at));
                }
                value = frames.pop().map_or(PythonJsonValue::Null, Frame::into_value);
                next = at + 1;
            }
        }
    }

    /// A member name from its opening quote at `at`, and the index of its value past the `:` and whitespace.
    fn key(&self, at: usize) -> Result<(PythonString, usize), PythonJsonError> {
        if !self.is(at, '"') {
            return Err(self.error(JsonDecodeErrorKind::ExpectingPropertyName, at));
        }
        let (key, at) = self.string(at + 1)?;
        let at = self.whitespace(at);
        if !self.is(at, ':') {
            return Err(self.error(JsonDecodeErrorKind::ExpectingColon, at));
        }
        Ok((key, self.whitespace(at + 1)))
    }

    /// A constant, or else a number.
    fn scalar(&self, index: usize) -> Result<(PythonJsonValue, usize), PythonJsonError> {
        let constants = [
            ("null", PythonJsonValue::Null),
            ("true", PythonJsonValue::Bool(true)),
            ("false", PythonJsonValue::Bool(false)),
            ("NaN", PythonJsonValue::Number(PythonNumber::Float(f64::NAN))),
            ("Infinity", PythonJsonValue::Number(PythonNumber::Float(f64::INFINITY))),
            (
                "-Infinity",
                PythonJsonValue::Number(PythonNumber::Float(f64::NEG_INFINITY)),
            ),
        ];
        for (word, value) in constants {
            if self.starts_with(index, word) {
                return Ok((value, index + word.len()));
            }
        }
        self.number(index)
    }

    /// `_match_number_unicode`: an `int` unless a fraction or an exponent follows. A `.` without a digit after it
    /// and an exponent without a digit are left for the caller to reject, and `e` or a sign as the last code point
    /// of the text is never part of the number.
    fn number(&self, start: usize) -> Result<(PythonJsonValue, usize), PythonJsonError> {
        let mut index = start + usize::from(self.is(start, '-'));
        if self.is(index, '0') {
            index += 1;
        } else if self.is_digit(index) {
            index = self.digits(index + 1);
        } else {
            return Err(self.error(JsonDecodeErrorKind::ExpectingValue, start));
        }
        let mut float = false;
        if self.is(index, '.') && self.is_digit(index + 1) {
            float = true;
            index = self.digits(index + 2);
        }
        if (self.is(index, 'e') || self.is(index, 'E')) && index + 1 < self.text.len() {
            let mut exponent = index + 1;
            if (self.is(exponent, '+') || self.is(exponent, '-')) && exponent + 1 < self.text.len() {
                exponent += 1;
            }
            let end = self.digits(exponent);
            if end > exponent {
                float = true;
                index = end;
            }
        }
        let spelling: String = self.text[start..index].iter().filter_map(|c| c.as_char()).collect();
        let number = if float {
            PythonNumber::Float(spelling.parse().unwrap_or(f64::NAN))
        } else {
            PythonNumber::Int(spelling.parse().map_err(PythonJsonError::Integer)?)
        };
        Ok((PythonJsonValue::Number(number), index))
    }

    /// `scanstring_unicode` from the code point after the opening quote: the string and the index past its
    /// closing quote.
    fn string(&self, mut end: usize) -> Result<(PythonString, usize), PythonJsonError> {
        let begin = end - 1;
        let mut string = PythonString::new();
        loop {
            let mut next = end;
            while next < self.text.len() && !self.is(next, '"') && !self.is(next, '\\') {
                if self.text[next] < PythonCodePoint::from(' ') {
                    return Err(self.error(JsonDecodeErrorKind::InvalidControlCharacter, next));
                }
                next += 1;
            }
            if next == self.text.len() {
                return Err(self.error(JsonDecodeErrorKind::UnterminatedString, begin));
            }
            string.extend(self.text[end..next].iter().copied());
            if self.is(next, '"') {
                return Ok((string, next + 1));
            }
            let next = next + 1;
            let Some(escape) = self.text.get(next) else {
                return Err(self.error(JsonDecodeErrorKind::UnterminatedString, begin));
            };
            if *escape != PythonCodePoint::from('u') {
                end = next + 1;
                let unescaped = match escape.as_char() {
                    Some(c @ ('"' | '\\' | '/')) => c,
                    Some('b') => '\u{8}',
                    Some('f') => '\u{c}',
                    Some('n') => '\n',
                    Some('r') => '\r',
                    Some('t') => '\t',
                    _ => return Err(self.error(JsonDecodeErrorKind::InvalidEscape, end - 2)),
                };
                string.push(PythonCodePoint::from(unescaped));
                continue;
            }
            let next = next + 1;
            end = next + 4;
            if end >= self.text.len() {
                return Err(self.error(JsonDecodeErrorKind::InvalidUnicodeEscape, next - 1));
            }
            let mut code_point = PythonCodePoint::from(self.hex(next)?);
            if code_point.is_high_surrogate()
                && end + 6 < self.text.len()
                && self.is(end, '\\')
                && self.is(end + 1, 'u')
            {
                let low = PythonCodePoint::from(self.hex(end + 2)?);
                if low.is_low_surrogate() {
                    code_point = PythonCodePoint::from_surrogates(code_point, low);
                    end += 6;
                }
            }
            string.push(code_point);
        }
    }

    /// The four hex digits from `at`, which the caller checked the text holds, raising at the `u` before them.
    fn hex(&self, at: usize) -> Result<u16, PythonJsonError> {
        self.text[at..at + 4].iter().try_fold(0_u16, |unit, c| {
            let digit = c
                .as_char()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.error(JsonDecodeErrorKind::InvalidUnicodeEscape, at - 1))?;
            Ok(unit * 16 + digit as u16)
        })
    }
}

#[cfg(test)]
#[path = "python_json_load_tests.rs"]
mod tests;
