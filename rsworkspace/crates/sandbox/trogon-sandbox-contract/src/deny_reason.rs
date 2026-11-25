use std::fmt;

use serde::{Deserialize, Serialize};

use crate::DenyReasonError;

/// Why a destination was refused, in words safe to put on an HTTP status line.
///
/// Construction and deserialization replace each run of control, invisible, and bidirectional formatting characters
/// with one space, drop `<` and `>`, trim the ends, and keep at most [`DenyReason::MAX_UTF16_UNITS`] UTF-16 code
/// units. A reason with nothing left is rejected. It serializes as the cleaned string.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DenyReason(String);

impl DenyReason {
    /// Longest reason kept, in UTF-16 code units.
    pub const MAX_UTF16_UNITS: usize = 500;

    pub fn new(raw: &str) -> Result<Self, DenyReasonError> {
        if raw.is_empty() {
            return Err(DenyReasonError::Empty);
        }
        let mut spaced = String::with_capacity(raw.len());
        let mut in_unsafe_run = false;
        for c in raw.chars() {
            if is_display_unsafe(c) {
                if !in_unsafe_run {
                    spaced.push(' ');
                }
                in_unsafe_run = true;
            } else {
                in_unsafe_run = false;
                if c != '<' && c != '>' {
                    spaced.push(c);
                }
            }
        }
        let mut units = 0;
        let cut: String = spaced
            .trim()
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= Self::MAX_UTF16_UNITS
            })
            .collect();
        let sanitized = cut.trim_end();
        if sanitized.is_empty() {
            return Err(DenyReasonError::Empty);
        }
        Ok(Self(sanitized.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_display_unsafe(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}'
        | '\u{7f}'..='\u{9f}'
        | '\u{ad}'
        | '\u{61c}'
        | '\u{200b}'..='\u{200f}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}'
        | '\u{e0000}'..='\u{e007f}')
}

impl TryFrom<String> for DenyReason {
    type Error = DenyReasonError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<DenyReason> for String {
    fn from(value: DenyReason) -> Self {
        value.0
    }
}

impl fmt::Display for DenyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
