//! Python `UnicodeDecodeError` as the `utf-8` codec raises it.

use std::{fmt, str::Utf8Error};

/// Why the `utf-8` codec stopped: the `reason` of a Python `UnicodeDecodeError`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnicodeDecodeReason {
    /// `invalid start byte`: a byte no UTF-8 sequence starts with.
    InvalidStartByte,
    /// `invalid continuation byte`: a sequence broken before its last byte.
    InvalidContinuationByte,
    /// `unexpected end of data`: the bytes end inside a sequence.
    UnexpectedEndOfData,
}

impl UnicodeDecodeReason {
    /// The `reason` text Python gives.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidStartByte => "invalid start byte",
            Self::InvalidContinuationByte => "invalid continuation byte",
            Self::UnexpectedEndOfData => "unexpected end of data",
        }
    }
}

/// Python `UnicodeDecodeError` from the `utf-8` codec: `object` holds the bytes the codec decoded, and `start..end`
/// the first bytes it could not. `Display` is `str()` of the Python exception.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub struct PythonUnicodeDecodeError {
    object: Vec<u8>,
    start: usize,
    end: usize,
    reason: UnicodeDecodeReason,
}

impl PythonUnicodeDecodeError {
    /// The error the `utf-8` codec raises for `object`, which Rust rejected with `error`.
    pub(crate) fn from_utf8_error(object: &[u8], error: Utf8Error) -> Self {
        let start = error.valid_up_to();
        let first = object.get(start).copied().unwrap_or_default();
        let (end, reason) = match error.error_len() {
            None => (object.len(), UnicodeDecodeReason::UnexpectedEndOfData),
            Some(1) if matches!(first, 0x80..=0xC1 | 0xF5..=0xFF) => {
                (start.saturating_add(1), UnicodeDecodeReason::InvalidStartByte)
            }
            Some(length) => (
                start.saturating_add(length),
                UnicodeDecodeReason::InvalidContinuationByte,
            ),
        };
        Self {
            object: object.to_vec(),
            start,
            end,
            reason,
        }
    }

    /// The `encoding` Python names: always `utf-8`.
    pub fn encoding(&self) -> &'static str {
        "utf-8"
    }

    /// The bytes the codec was asked to decode, after any byte order mark the `utf-8-sig` codec dropped.
    pub fn object(&self) -> &[u8] {
        &self.object
    }

    /// Where in [`object`](Self::object) the bytes that cannot be decoded start.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Where in [`object`](Self::object) the bytes that cannot be decoded end, exclusive.
    pub fn end(&self) -> usize {
        self.end
    }

    pub fn reason(&self) -> UnicodeDecodeReason {
        self.reason
    }
}

impl fmt::Display for PythonUnicodeDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = self.reason.as_str();
        if self.end.saturating_sub(self.start) == 1 {
            let byte = self.object.get(self.start).copied().unwrap_or_default();
            write!(
                f,
                "'utf-8' codec can't decode byte 0x{byte:02x} in position {}: {reason}",
                self.start
            )
        } else {
            write!(
                f,
                "'utf-8' codec can't decode bytes in position {}-{}: {reason}",
                self.start,
                self.end.saturating_sub(1)
            )
        }
    }
}
