use crate::constants::MAX_IDENTIFIER_BYTES;

/// Why a string cannot be an exact identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentifierError {
    /// Identifiers cannot be empty.
    #[error("identifier cannot be empty")]
    Empty,
    /// Identifiers are bounded so a binding cannot be made arbitrarily large.
    #[error("identifier is {actual} bytes, the limit is {limit}")]
    TooLong {
        /// The length that was supplied.
        actual: usize,
        /// The longest accepted length.
        limit: usize,
    },
    /// Whitespace and control characters make an identity ambiguous in logs
    /// and in the audit trail.
    #[error("identifier cannot contain whitespace or control characters")]
    ContainsWhitespaceOrControl,
}

pub(crate) fn validate_identifier(value: &str) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty);
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(IdentifierError::TooLong {
            actual: value.len(),
            limit: MAX_IDENTIFIER_BYTES,
        });
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(IdentifierError::ContainsWhitespaceOrControl);
    }
    Ok(())
}

/// Declares an exact-identity string value object.
macro_rules! identifier_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Creates the identity after rejecting an empty, oversized, or ambiguous value.
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::identifier::IdentifierError> {
                let value = value.into();
                $crate::identifier::validate_identifier(&value)?;
                Ok(Self(value))
            }

            /// Returns the identity as stored.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::identifier::IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }
    };
}

pub(crate) use identifier_type;
