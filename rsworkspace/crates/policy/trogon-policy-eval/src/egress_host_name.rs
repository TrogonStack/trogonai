use std::fmt;
use std::str::FromStr;

use crate::constants::{MAX_HOST_BYTES, MAX_HOST_LABEL_BYTES};

/// Why a string cannot be an exact egress host.
///
/// Every variant is a refusal at construction, so a malformed host can never
/// reach a comparison where it might match something it should not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EgressHostNameError {
    /// A host cannot be empty.
    #[error("host cannot be empty")]
    Empty,
    /// The whole name exceeds the DNS limit.
    #[error("host is {actual} bytes, the limit is {limit}")]
    TooLong {
        /// The length that was supplied.
        actual: usize,
        /// The longest accepted length.
        limit: usize,
    },
    /// A label between dots exceeds the DNS limit.
    #[error("host label is {actual} bytes, the limit is {limit}")]
    LabelTooLong {
        /// The length that was supplied.
        actual: usize,
        /// The longest accepted length.
        limit: usize,
    },
    /// Two dots in a row, or a leading dot.
    #[error("host cannot contain an empty label")]
    EmptyLabel,
    /// An absolute name with a trailing dot is a different spelling of the
    /// same host, and exact matching has one spelling.
    #[error("host cannot end with a dot")]
    TrailingDot,
    /// Hosts are lowercase. Folding case here would hide a distinction the
    /// policy author did not write down, so uppercase is refused instead.
    #[error("host must be lowercase")]
    Uppercase,
    /// Non-ASCII hosts are refused so a lookalike of a permitted host cannot
    /// be mistaken for it. Callers that accept internationalized names
    /// convert to the ASCII form before they get here.
    #[error("host must be ASCII")]
    NotAscii,
    /// A character that cannot appear in a plain host name, such as a
    /// wildcard, a scheme or path separator, a port separator, or whitespace.
    #[error("host cannot contain {character:?}")]
    InvalidCharacter {
        /// The first offending character.
        character: char,
    },
    /// A label cannot start or end with a hyphen.
    #[error("host label cannot start or end with a hyphen")]
    HyphenAtLabelEdge,
}

/// An exact host name, with no wildcard, scheme, path, port, or trailing dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EgressHostName(String);

impl EgressHostName {
    /// Creates a host after rejecting everything that is not a plain,
    /// lowercase, ASCII DNS name.
    pub fn new(value: impl Into<String>) -> Result<Self, EgressHostNameError> {
        let value = value.into();
        validate(&value)?;
        Ok(Self(value))
    }

    /// Returns the host as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate(value: &str) -> Result<(), EgressHostNameError> {
    if value.is_empty() {
        return Err(EgressHostNameError::Empty);
    }
    if value.len() > MAX_HOST_BYTES {
        return Err(EgressHostNameError::TooLong {
            actual: value.len(),
            limit: MAX_HOST_BYTES,
        });
    }
    if !value.is_ascii() {
        return Err(EgressHostNameError::NotAscii);
    }
    if value.ends_with('.') {
        return Err(EgressHostNameError::TrailingDot);
    }
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            return Err(EgressHostNameError::Uppercase);
        }
        if !(character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' || character == '.') {
            return Err(EgressHostNameError::InvalidCharacter { character });
        }
    }
    for label in value.split('.') {
        if label.is_empty() {
            return Err(EgressHostNameError::EmptyLabel);
        }
        if label.len() > MAX_HOST_LABEL_BYTES {
            return Err(EgressHostNameError::LabelTooLong {
                actual: label.len(),
                limit: MAX_HOST_LABEL_BYTES,
            });
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(EgressHostNameError::HyphenAtLabelEdge);
        }
    }
    Ok(())
}

impl fmt::Display for EgressHostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for EgressHostName {
    type Err = EgressHostNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
