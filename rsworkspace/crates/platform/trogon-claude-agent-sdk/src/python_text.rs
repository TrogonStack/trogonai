//! Python's view of text: the `str` it decodes from an OS string, `repr(str)`, and `int(str)` in base 10.

use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::constants::PYTHON_INT_MAX_STR_DIGITS;

/// One code point of a Python `str`, which may be a lone surrogate that Rust's `char` cannot hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonChar {
    Scalar(char),
    Surrogate(u16),
}

impl PythonChar {
    fn code(self) -> u32 {
        match self {
            Self::Scalar(character) => u32::from(character),
            Self::Surrogate(unit) => u32::from(unit),
        }
    }
}

pub(crate) fn from_str(text: &str) -> Vec<PythonChar> {
    text.chars().map(PythonChar::Scalar).collect()
}

/// The `str` Python reads from an OS string: POSIX decodes UTF-8 with `surrogateescape`.
#[cfg(unix)]
pub(crate) fn from_os_str(text: &OsStr) -> Vec<PythonChar> {
    from_bytes(text.as_bytes())
}

/// The `str` Python reads from an OS string: Windows keeps unpaired UTF-16 surrogates.
#[cfg(windows)]
pub(crate) fn from_os_str(text: &OsStr) -> Vec<PythonChar> {
    char::decode_utf16(text.encode_wide())
        .map(|unit| match unit {
            Ok(character) => PythonChar::Scalar(character),
            Err(error) => PythonChar::Surrogate(error.unpaired_surrogate()),
        })
        .collect()
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn from_os_str(text: &OsStr) -> Vec<PythonChar> {
    from_str(&text.to_string_lossy())
}

/// UTF-8 decoded with `surrogateescape`: every byte outside a valid sequence becomes `U+DC00 + byte`.
#[cfg(unix)]
fn from_bytes(bytes: &[u8]) -> Vec<PythonChar> {
    let mut text = Vec::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.extend(chunk.valid().chars().map(PythonChar::Scalar));
        text.extend(
            chunk
                .invalid()
                .iter()
                .map(|&byte| PythonChar::Surrogate(0xdc00 | u16::from(byte))),
        );
    }
    text
}

/// Python `repr(str)`.
pub(crate) fn repr(text: &[PythonChar]) -> String {
    let has = |character: char| text.contains(&PythonChar::Scalar(character));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for &character in text {
        match character {
            PythonChar::Scalar(character) if character == quote || character == '\\' => {
                out.push('\\');
                out.push(character);
            }
            PythonChar::Scalar('\t') => out.push_str("\\t"),
            PythonChar::Scalar('\n') => out.push_str("\\n"),
            PythonChar::Scalar('\r') => out.push_str("\\r"),
            PythonChar::Scalar(character) if character == ' ' || is_printable(character) => out.push(character),
            character => {
                let code = character.code();
                out.push_str(&if code < 0x100 {
                    format!("\\x{code:02x}")
                } else if code < 0x10000 {
                    format!("\\u{code:04x}")
                } else {
                    format!("\\U{code:08x}")
                });
            }
        }
    }
    out.push(quote);
    out
}

/// Python `str.isprintable` for one character: not in `Other` or `Separator`, except the space.
fn is_printable(character: char) -> bool {
    !matches!(
        get_general_category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::SpaceSeparator
    )
}

/// The integer Python's `int(text)` reads in base 10, kept as its sign and ASCII digits.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PythonInt {
    negative: bool,
    digits: String,
}

impl PythonInt {
    /// `float(int)`, or `None` where Python raises `OverflowError: int too large to convert to float`.
    pub(crate) fn to_f64(&self) -> Option<f64> {
        let magnitude: f64 = self.digits.parse().ok()?;
        magnitude
            .is_finite()
            .then_some(if self.negative { -magnitude } else { magnitude })
    }
}

/// Why Python's `int(text)` in base 10 refused a `str`.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum IntError {
    /// `invalid literal for int() with base 10: ...`
    InvalidLiteral,
    /// A well formed literal with more digits than the default integer string limit.
    TooManyDigits(usize),
}

/// Python `int(text)` in base 10: Unicode decimal digits and whitespace count as their ASCII forms, then an optional
/// sign and digits with single underscores between them, surrounded by ASCII whitespace.
pub(crate) fn int(text: &[PythonChar]) -> Result<PythonInt, IntError> {
    let ascii: Vec<u8> = text.iter().map(|&character| int_byte(character)).collect();
    let is_space = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r');
    let start = ascii.iter().position(|byte| !is_space(byte)).unwrap_or(ascii.len());
    let end = ascii
        .iter()
        .rposition(|byte| !is_space(byte))
        .map_or(start, |last| last + 1);
    let literal = &ascii[start..end];
    let negative = literal.first() == Some(&b'-');
    let body = match literal.first() {
        Some(b'+' | b'-') => &literal[1..],
        _ => literal,
    };
    let well_formed = body.first().is_some_and(u8::is_ascii_digit)
        && body.last().is_some_and(u8::is_ascii_digit)
        && body.iter().all(|byte| byte.is_ascii_digit() || *byte == b'_')
        && !body.windows(2).any(|pair| pair == b"__");
    if !well_formed {
        return Err(IntError::InvalidLiteral);
    }
    let digits: String = body
        .iter()
        .filter(|byte| byte.is_ascii_digit())
        .map(|&byte| char::from(byte))
        .collect();
    if digits.len() > PYTHON_INT_MAX_STR_DIGITS {
        return Err(IntError::TooManyDigits(digits.len()));
    }
    Ok(PythonInt { negative, digits })
}

/// CPython's `_PyUnicode_TransformDecimalAndSpaceToASCII` for one code point; `?` never parses.
fn int_byte(character: PythonChar) -> u8 {
    match character {
        PythonChar::Scalar(character) if character < '\u{7f}' => u8::try_from(character).unwrap_or(b'?'),
        PythonChar::Scalar(character) if is_space(character) => b' ',
        PythonChar::Scalar(character) => decimal(character).map_or(b'?', |digit| b'0' + digit),
        PythonChar::Surrogate(_) => b'?',
    }
}

/// `str.isspace` above ASCII.
fn is_space(character: char) -> bool {
    matches!(
        character,
        '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `unicodedata.decimal`: every decimal digit sits in a run of ten that starts at zero, and the runs that touch
/// start at multiples of ten from the first one.
fn decimal(character: char) -> Option<u8> {
    let is_decimal = |character: char| matches!(get_general_category(character), GeneralCategory::DecimalNumber);
    if !is_decimal(character) {
        return None;
    }
    let code = u32::from(character);
    let mut start = code;
    while let Some(previous) = start.checked_sub(1).and_then(char::from_u32)
        && is_decimal(previous)
    {
        start -= 1;
    }
    u8::try_from((code - start) % 10).ok()
}

#[cfg(test)]
#[path = "python_text_tests.rs"]
mod tests;
