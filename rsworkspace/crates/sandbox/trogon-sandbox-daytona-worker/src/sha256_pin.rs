use sha2::{Digest as _, Sha256};

use crate::OrdinaryCommandRuntimeError;

/// A SHA-256 digest as 64 lowercase hex digits.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Sha256Pin(String);

impl Sha256Pin {
    /// Accepts exactly 64 lowercase hex digits.
    pub(crate) fn parse(value: &str) -> Result<Self, OrdinaryCommandRuntimeError> {
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(OrdinaryCommandRuntimeError::PinNotLowercaseHex)
        }
    }

    /// The digest of `bytes`.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
