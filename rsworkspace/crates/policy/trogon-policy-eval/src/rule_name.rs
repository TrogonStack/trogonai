use std::fmt;

use crate::constants::MAX_RULE_NAME_BYTES;

/// Why a string cannot name a request rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuleNameError {
    /// A refusal that names nothing cannot be explained.
    #[error("rule name must not be empty or whitespace-only")]
    Empty,
    /// Rule names are bounded so a refusal stays small.
    #[error("rule name is {actual} bytes, the limit is {limit}")]
    TooLong {
        /// The length that was supplied.
        actual: usize,
        /// The longest accepted length.
        limit: usize,
    },
    /// Control characters would corrupt logs and audit text.
    #[error("rule name cannot contain control characters")]
    ContainsControlCharacter,
}

/// The name of one request rule, as it appears in a refusal.
///
/// Interior spaces are allowed because rule names are authored by operators
/// and often come from file names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleName(String);

impl RuleName {
    /// Creates a rule name after rejecting an unusable value.
    pub fn new(value: impl Into<String>) -> Result<Self, RuleNameError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RuleNameError::Empty);
        }
        if value.len() > MAX_RULE_NAME_BYTES {
            return Err(RuleNameError::TooLong {
                actual: value.len(),
                limit: MAX_RULE_NAME_BYTES,
            });
        }
        if value.chars().any(char::is_control) {
            return Err(RuleNameError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
