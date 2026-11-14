//! The exceptions Python's `json.loads` raises for text it refuses.

use crate::python_code_point::PythonCodePoint;

/// Why [`python_json_loads`](crate::python_json_loads) refused text.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PythonJsonError {
    /// `json.JSONDecodeError`.
    #[error(transparent)]
    Decode(JsonDecodeError),
    /// `RecursionError`: objects and arrays nested deeper than the limit, raised on entering the container that
    /// goes past it.
    #[error("maximum recursion depth exceeded while decoding a JSON {} from a unicode string", .0.name())]
    Recursion(JsonContainer),
    /// `ValueError`: an integer token has more digits than Python converts.
    #[error(transparent)]
    Integer(ExactIntegerError),
}

/// `json.JSONDecodeError`: what the scanner expected, and where. Positions count code points, as Python indexes
/// a `str`.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{}: line {lineno} column {colno} (char {pos})", .kind.message())]
pub struct JsonDecodeError {
    kind: JsonDecodeErrorKind,
    pos: usize,
    lineno: usize,
    colno: usize,
}

impl JsonDecodeError {
    /// The error `JSONDecodeError(msg, doc, pos)` builds, with the line and column of `pos` in `text`.
    pub(crate) fn new(kind: JsonDecodeErrorKind, text: &[PythonCodePoint], pos: usize) -> Self {
        let newline = PythonCodePoint::from('\n');
        let before = &text[..pos.min(text.len())];
        let lineno = before.iter().filter(|c| **c == newline).count() + 1;
        let colno = match before.iter().rposition(|c| *c == newline) {
            Some(line_start) => pos - line_start,
            None => pos + 1,
        };
        Self {
            kind,
            pos,
            lineno,
            colno,
        }
    }

    /// What the scanner expected, which is the unformatted message `msg`.
    pub fn kind(&self) -> JsonDecodeErrorKind {
        self.kind
    }

    /// The code point index where parsing failed.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The line of [`pos`](Self::pos), counted from 1.
    pub fn lineno(&self) -> usize {
        self.lineno
    }

    /// The column of [`pos`](Self::pos), counted from 1.
    pub fn colno(&self) -> usize {
        self.colno
    }
}

/// The unformatted message of a [`JsonDecodeError`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonDecodeErrorKind {
    /// No value starts here.
    ExpectingValue,
    /// An object key is not a string.
    ExpectingPropertyName,
    /// An object key is not followed by `:`.
    ExpectingColon,
    /// An array item or object member is followed by neither `,` nor the closing bracket.
    ExpectingComma,
    /// A string has no closing quote; the position is its opening quote.
    UnterminatedString,
    /// A string holds a character below U+0020.
    InvalidControlCharacter,
    /// A backslash is followed by a letter that is no escape; the position is the backslash.
    InvalidEscape,
    /// A `\u` escape lacks four hex digits; the position is the `u`.
    InvalidUnicodeEscape,
    /// Text follows the document.
    ExtraData,
    /// The text starts with U+FEFF, which `json.loads` refuses for a `str`.
    ByteOrderMark,
}

impl JsonDecodeErrorKind {
    /// The message as CPython 3.12 spells it.
    pub fn message(self) -> &'static str {
        match self {
            Self::ExpectingValue => "Expecting value",
            Self::ExpectingPropertyName => "Expecting property name enclosed in double quotes",
            Self::ExpectingColon => "Expecting ':' delimiter",
            Self::ExpectingComma => "Expecting ',' delimiter",
            Self::UnterminatedString => "Unterminated string starting at",
            Self::InvalidControlCharacter => "Invalid control character at",
            Self::InvalidEscape => "Invalid \\escape",
            Self::InvalidUnicodeEscape => "Invalid \\uXXXX escape",
            Self::ExtraData => "Extra data",
            Self::ByteOrderMark => "Unexpected UTF-8 BOM (decode using utf-8-sig)",
        }
    }
}

/// The kind of container a [`PythonJsonError::Recursion`] was raised on entering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonContainer {
    Object,
    Array,
}

impl JsonContainer {
    /// The name the `RecursionError` message gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Array => "array",
        }
    }
}

/// Why text is not an [`ExactInteger`](crate::ExactInteger). The messages are the ones Python's `int()` raises.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ExactIntegerError {
    #[error("invalid literal for int() with base 10: {0}")]
    Invalid(String),
    #[error(
        "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    )]
    TooManyDigits { limit: usize, digits: usize },
}

#[cfg(test)]
#[path = "python_json_error_tests.rs"]
mod tests;
