use std::borrow::Borrow;

use crate::constants::{MAX_FACT_NAME_BYTES, RESERVED_WORDS};

/// Why a string cannot name a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FactNameError {
    /// A fact needs a name to be referenced by.
    #[error("fact name must not be empty")]
    Empty,
    /// Names are bounded.
    #[error("fact name is {actual} bytes, the limit is {limit}")]
    TooLong {
        /// The length that was supplied.
        actual: usize,
        /// The longest accepted length.
        limit: usize,
    },
    /// A name must be usable as an identifier in a rule.
    #[error("fact name cannot contain {character:?}")]
    InvalidCharacter {
        /// The first character that is not allowed.
        character: char,
    },
    /// An identifier cannot begin with a digit.
    #[error("fact name cannot start with a digit")]
    StartsWithDigit,
    /// The language reserves the word.
    #[error("fact name is a reserved word of the rule language")]
    Reserved,
}

/// The name a rule uses to read one fact, such as `request` or `agent`.
///
/// It is a plain identifier: ASCII letters, digits and underscores, not
/// starting with a digit, and not a word the language reserves. A name that
/// could not be written in a rule could never be read, so it is refused where
/// it is created.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FactName(String);

impl FactName {
    /// Creates a fact name after rejecting an unusable value.
    pub fn new(value: impl Into<String>) -> Result<Self, FactNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(FactNameError::Empty);
        }
        if value.len() > MAX_FACT_NAME_BYTES {
            return Err(FactNameError::TooLong {
                actual: value.len(),
                limit: MAX_FACT_NAME_BYTES,
            });
        }
        if let Some(character) = value.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '_')) {
            return Err(FactNameError::InvalidCharacter { character });
        }
        if value.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(FactNameError::StartsWithDigit);
        }
        if RESERVED_WORDS.contains(&value.as_str()) {
            return Err(FactNameError::Reserved);
        }
        Ok(Self(value))
    }

    /// Returns the name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for FactName {
    fn borrow(&self) -> &str {
        &self.0
    }
}
