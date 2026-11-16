//! One code point of a Python `str`: anything up to U+10FFFF, surrogates included, which a Rust `char` excludes.

/// A code point up to U+10FFFF, which may be a surrogate. Code points order by their numbers, as Python compares
/// the characters of two strings.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PythonCodePoint {
    value: u32,
}

/// Why a number is not a [`PythonCodePoint`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PythonCodePointError {
    /// The number is past U+10FFFF, which no Python `str` holds.
    #[error("code point {0:#x} not in range(0x110000)")]
    OutOfRange(u32),
}

impl PythonCodePoint {
    /// The code point a UTF-16 surrogate pair decodes to, `high` being a high surrogate and `low` a low one.
    pub(crate) fn from_surrogates(high: Self, low: Self) -> Self {
        Self {
            value: 0x1_0000 + ((high.value & 0x3FF) << 10) + (low.value & 0x3FF),
        }
    }

    /// Whether this is a high surrogate, U+D800 to U+DBFF, which leads a UTF-16 pair.
    pub(crate) fn is_high_surrogate(self) -> bool {
        (0xD800..0xDC00).contains(&self.value)
    }

    /// Whether this is a low surrogate, U+DC00 to U+DFFF, which ends a UTF-16 pair.
    pub(crate) fn is_low_surrogate(self) -> bool {
        (0xDC00..0xE000).contains(&self.value)
    }

    /// The code point as a number, which Python's `ord` returns.
    pub fn as_u32(self) -> u32 {
        self.value
    }

    /// The code point as a `char`, or `None` for a surrogate.
    pub fn as_char(self) -> Option<char> {
        char::from_u32(self.value)
    }
}

impl From<char> for PythonCodePoint {
    fn from(c: char) -> Self {
        Self { value: u32::from(c) }
    }
}

/// A UTF-16 code unit read on its own, a lone surrogate included.
impl From<u16> for PythonCodePoint {
    fn from(unit: u16) -> Self {
        Self { value: u32::from(unit) }
    }
}

impl TryFrom<u32> for PythonCodePoint {
    type Error = PythonCodePointError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value > 0x10_FFFF {
            return Err(PythonCodePointError::OutOfRange(value));
        }
        Ok(Self { value })
    }
}

#[cfg(test)]
#[path = "python_code_point_tests.rs"]
mod tests;
