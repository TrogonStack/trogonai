//! NATS-safe subject token value objects.
//!
//! Two concrete types cover the two validation flavors used in practice:
//!
//! - [`NatsToken`] — single token, no dots, ASCII-only, max 128 chars
//! - [`DottedNatsToken`] — dotted segments allowed, UTF-8, max 128 bytes
//!
//! All validation happens at construction; invalid instances are unrepresentable.

use std::sync::Arc;

use crate::constants::MAX_NATS_TOKEN_LENGTH;
use crate::subject_token_violation::SubjectTokenViolation;
use crate::token;

/// A validated single NATS subject token.
///
/// Rejects empty, non-ASCII, dots, wildcards (`*`, `>`), and whitespace.
/// Max 128 characters. Wraps an `Arc<str>` so cloning is cheap.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NatsToken(Arc<str>);

impl NatsToken {
    /// Validate and construct a new single token.
    pub fn new(s: impl AsRef<str>) -> Result<Self, SubjectTokenViolation> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(SubjectTokenViolation::Empty);
        }
        let mut char_count: usize = 0;
        for ch in s.chars() {
            char_count += 1;
            if char_count > MAX_NATS_TOKEN_LENGTH {
                return Err(SubjectTokenViolation::TooLong(char_count));
            }
            if !ch.is_ascii() || ch == '.' || ch == '*' || ch == '>' || ch.is_whitespace() {
                return Err(SubjectTokenViolation::InvalidCharacter(ch));
            }
        }
        Ok(Self(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NatsToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for NatsToken {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for NatsToken {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A validated dotted NATS subject segment.
///
/// Allows `.` as a token separator but rejects malformed dots (consecutive,
/// leading, trailing). Rejects wildcards (`*`, `>`) and whitespace.
/// Max 128 bytes. Wraps an `Arc<str>` so cloning is cheap.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DottedNatsToken(Arc<str>);

impl DottedNatsToken {
    /// Validate and construct a new dotted token.
    pub fn new(s: impl AsRef<str>) -> Result<Self, SubjectTokenViolation> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(SubjectTokenViolation::Empty);
        }
        if let Some(ch) = token::has_wildcards_or_whitespace(s) {
            return Err(SubjectTokenViolation::InvalidCharacter(ch));
        }
        if token::has_consecutive_or_boundary_dots(s) {
            return Err(SubjectTokenViolation::InvalidCharacter('.'));
        }
        if s.len() > MAX_NATS_TOKEN_LENGTH {
            return Err(SubjectTokenViolation::TooLong(s.len()));
        }
        Ok(Self(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DottedNatsToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for DottedNatsToken {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for DottedNatsToken {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
