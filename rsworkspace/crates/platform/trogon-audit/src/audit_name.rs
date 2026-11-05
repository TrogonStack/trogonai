//! A name a record may carry: a resource name, a host, a method, a rule id.
//!
//! Records hold no secret, token, prompt, tool argument, header value or body
//! (ADR#0086 Decision 5). The record types therefore have no string field a caller
//! fills freely: every one is an [`AuditName`], a [`ReasonCode`](crate::ReasonCode)
//! or an identifier from the generated types. A name is short and drawn from the
//! characters a resource name, a host or a method uses, so the shapes content takes
//! (a header line, a JSON body, a prompt, a query string) are refused at the door.
//!
//! This is a guard on shape, not a classifier: a bare token such as an API key also
//! has a name's shape. The structural guarantee is that no API of this crate takes
//! content at all, so a caller has nowhere honest to put one.

use crate::constants::{MAX_NAME_BYTES, NAME_PUNCTUATION};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuditName(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuditNameError {
    #[error("audit name is empty")]
    Empty,
    #[error("audit name is {0} bytes, over the {MAX_NAME_BYTES}-byte limit")]
    TooLong(usize),
    /// Names the character and never the value, which may be the content the
    /// rule exists to keep out.
    #[error("audit name contains a character a name does not use: {0:?}")]
    InvalidCharacter(char),
}

impl AuditName {
    pub fn new(value: impl AsRef<str>) -> Result<Self, AuditNameError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(AuditNameError::Empty);
        }
        if value.len() > MAX_NAME_BYTES {
            return Err(AuditNameError::TooLong(value.len()));
        }
        if let Some(character) = value
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(*c)))
        {
            return Err(AuditNameError::InvalidCharacter(character));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AuditName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
