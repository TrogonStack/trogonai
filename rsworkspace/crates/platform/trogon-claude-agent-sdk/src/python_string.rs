//! A Python `str`: any sequence of code points up to U+10FFFF. Unlike a Rust `String` it can hold a lone
//! surrogate, which `json.loads` returns for a `"\ud800"` escape.

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::{
    borrow::Cow,
    fmt::{self, Write as _},
    hash::{Hash, Hasher},
};

use indexmap::Equivalent;

use crate::python_code_point::PythonCodePoint;

/// The code points of a Python `str`, in order. Strings order by their code points, as Python compares them: the
/// encoding below keeps that order byte for byte.
#[derive(Clone, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct PythonString {
    /// UTF-8, except that a surrogate is encoded on its own in three bytes, as UTF-8 encodes the code points
    /// around it.
    bytes: Vec<u8>,
}

impl PythonString {
    /// The empty string.
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// `bytes.decode('utf-8', 'surrogateescape')`: each byte outside a valid UTF-8 sequence becomes the surrogate
    /// U+DC00 plus the byte.
    pub(crate) fn decode_surrogateescape(bytes: &[u8]) -> Self {
        let mut text = Self::new();
        for chunk in bytes.utf8_chunks() {
            text.push_str(chunk.valid());
            for &byte in chunk.invalid() {
                text.push(PythonCodePoint::from(0xDC00 | u16::from(byte)));
            }
        }
        text
    }

    /// An `os.environ` name or value: the bytes decoded with `surrogateescape` on Unix, as CPython decodes them with
    /// the UTF-8 file system encoding.
    #[cfg(unix)]
    pub(crate) fn from_os(text: &std::ffi::OsStr) -> Self {
        Self::decode_surrogateescape(text.as_bytes())
    }

    /// An `os.environ` name or value: the UTF-16 of Windows, each unpaired surrogate kept.
    #[cfg(windows)]
    pub(crate) fn from_os(text: &std::ffi::OsStr) -> Self {
        char::decode_utf16(text.encode_wide())
            .map(|decoded| match decoded {
                Ok(c) => PythonCodePoint::from(c),
                Err(error) => PythonCodePoint::from(error.unpaired_surrogate()),
            })
            .collect()
    }

    /// An `os.environ` name or value, where the platform has no encoding to undo.
    #[cfg(not(any(unix, windows)))]
    pub(crate) fn from_os(text: &std::ffi::OsStr) -> Self {
        Self::from(&*text.to_string_lossy())
    }

    /// The string with `map` applied to each run of code points between the surrogates, which stay as they are.
    /// Exact for `str.lower()` and `str.upper()`: a surrogate is neither cased nor case-ignorable, so the context
    /// of a final sigma ends at it.
    pub(crate) fn map_text(&self, map: impl Fn(&str) -> String) -> Self {
        let mut text = Self::new();
        for chunk in self.bytes.utf8_chunks() {
            text.push_str(&map(chunk.valid()));
            text.bytes.extend_from_slice(chunk.invalid());
        }
        text
    }

    /// `text.strip(chars)` for the code points `strip` matches; a surrogate is never stripped.
    pub(crate) fn trim_matches(&self, strip: impl Fn(char) -> bool) -> Self {
        let code_points: Vec<PythonCodePoint> = self.code_points().collect();
        let kept = |code_point: &PythonCodePoint| !code_point.as_char().is_some_and(&strip);
        let start = code_points.iter().position(kept).unwrap_or(code_points.len());
        let end = code_points.iter().rposition(kept).map_or(start, |last| last + 1);
        code_points
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .copied()
            .collect()
    }

    /// `text.encode('utf-8')`: the text, or the `str` of the `UnicodeEncodeError` for the first run of surrogates,
    /// with its positions in code points.
    pub(crate) fn encode_utf8(&self) -> Result<&str, String> {
        if let Some(text) = self.as_str() {
            return Ok(text);
        }
        let code_points: Vec<PythonCodePoint> = self.code_points().collect();
        let start = code_points
            .iter()
            .position(|code_point| code_point.as_char().is_none())
            .unwrap_or(0);
        let run = code_points
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take_while(|code_point| code_point.as_char().is_none())
            .count();
        Err(match code_points.get(start) {
            Some(code_point) if run == 1 => format!(
                "'utf-8' codec can't encode character '\\u{:04x}' in position {start}: surrogates not allowed",
                code_point.as_u32()
            ),
            _ => format!(
                "'utf-8' codec can't encode characters in position {start}-{}: surrogates not allowed",
                start + run.saturating_sub(1)
            ),
        })
    }

    /// Appends `code_point`, which may be a surrogate.
    pub fn push(&mut self, code_point: PythonCodePoint) {
        match code_point.as_char() {
            Some(c) => self.bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None => {
                let value = code_point.as_u32();
                self.bytes.extend([
                    0xE0 | (value >> 12) as u8,
                    0x80 | ((value >> 6) & 0x3F) as u8,
                    0x80 | (value & 0x3F) as u8,
                ]);
            }
        }
    }

    /// Appends `text`.
    pub fn push_str(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
    }

    /// The string as a `String`, or the string back when it holds a surrogate.
    pub fn into_string(self) -> Result<String, Self> {
        String::from_utf8(self.bytes).map_err(|error| Self {
            bytes: error.into_bytes(),
        })
    }

    /// The code points, in order.
    pub fn code_points(&self) -> CodePoints<'_> {
        CodePoints { bytes: &self.bytes }
    }

    /// The string as a `&str`, or `None` when it holds a surrogate.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Whether the string holds no code point.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `len(text)`: the number of code points.
    pub fn len(&self) -> usize {
        self.code_points().count()
    }

    /// `needle in text`. Every code point has one encoding and no encoding starts inside another, so a run of
    /// code points is found exactly where its bytes are.
    pub fn contains(&self, needle: &Self) -> bool {
        needle.bytes.is_empty()
            || self
                .bytes
                .windows(needle.bytes.len())
                .any(|window| window == needle.bytes)
    }

    /// The string as UTF-8, each lone surrogate replaced by U+FFFD, for the code that still reads text as a Rust
    /// `str`. It borrows, and is exact, for every string without a surrogate.
    pub(crate) fn to_str_lossy(&self) -> Cow<'_, str> {
        match self.as_str() {
            Some(text) => Cow::Borrowed(text),
            None => Cow::Owned(
                self.code_points()
                    .map(|code_point| code_point.as_char().unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect(),
            ),
        }
    }

    /// The encoding: UTF-8, with each lone surrogate in three bytes of its own. A byte below `0x80` is always a
    /// whole ASCII code point, so cutting at ASCII bytes keeps every code point whole.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The string of an encoding from [`PythonString::as_bytes`], or of such encodings cut and joined at ASCII bytes.
    pub(crate) fn from_encoded(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// Hashes as the `str` holding the same text does, so that a map keyed by [`PythonString`] can be looked up by a
/// `&str`.
impl Hash for PythonString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.as_str() {
            Some(text) => text.hash(state),
            None => {
                state.write(&self.bytes);
                state.write_u8(0xFF);
            }
        }
    }
}

/// A `&str` finds the [`PythonString`] key holding the same text.
impl Equivalent<PythonString> for str {
    fn equivalent(&self, key: &PythonString) -> bool {
        self.as_bytes() == key.bytes
    }
}

/// The code points of a [`PythonString`], from [`PythonString::code_points`].
#[derive(Clone, Debug)]
pub struct CodePoints<'s> {
    bytes: &'s [u8],
}

impl Iterator for CodePoints<'_> {
    type Item = PythonCodePoint;

    fn next(&mut self) -> Option<PythonCodePoint> {
        let lead = *self.bytes.first()?;
        let (width, bits) = match lead {
            0x00..=0x7F => (1, lead),
            0xC0..=0xDF => (2, lead & 0x1F),
            0xE0..=0xEF => (3, lead & 0x0F),
            _ => (4, lead & 0x07),
        };
        let (encoded, rest) = self.bytes.split_at_checked(width)?;
        self.bytes = rest;
        let value = encoded[1..]
            .iter()
            .fold(u32::from(bits), |value, byte| (value << 6) | u32::from(byte & 0x3F));
        PythonCodePoint::try_from(value).ok()
    }
}

impl Extend<PythonCodePoint> for PythonString {
    fn extend<I: IntoIterator<Item = PythonCodePoint>>(&mut self, code_points: I) {
        for code_point in code_points {
            self.push(code_point);
        }
    }
}

impl FromIterator<PythonCodePoint> for PythonString {
    fn from_iter<I: IntoIterator<Item = PythonCodePoint>>(code_points: I) -> Self {
        let mut text = Self::new();
        text.extend(code_points);
        text
    }
}

impl From<PythonCodePoint> for PythonString {
    fn from(code_point: PythonCodePoint) -> Self {
        let mut text = Self::new();
        text.push(code_point);
        text
    }
}

impl From<&str> for PythonString {
    fn from(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
        }
    }
}

impl From<String> for PythonString {
    fn from(text: String) -> Self {
        Self {
            bytes: text.into_bytes(),
        }
    }
}

/// Quoted like a `str`'s `Debug`, with each surrogate as `\u{d800}`.
impl fmt::Debug for PythonString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_char('"')?;
        for code_point in self.code_points() {
            match code_point.as_char() {
                Some('\'') => formatter.write_char('\'')?,
                Some(c) => write!(formatter, "{}", c.escape_debug())?,
                None => write!(formatter, "\\u{{{:x}}}", code_point.as_u32())?,
            }
        }
        formatter.write_char('"')
    }
}

#[cfg(test)]
#[path = "python_string_tests.rs"]
mod tests;
