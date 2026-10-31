use std::fmt;

use super::constants::KEY_ID_LEN_MAX;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaidKeyIdError {
    #[error("key id must not be empty")]
    Empty,
    #[error("key id is too long: {0} bytes (max {max})", max = KEY_ID_LEN_MAX)]
    TooLong(usize),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PlaidKeyId(String);

impl PlaidKeyId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, PlaidKeyIdError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(PlaidKeyIdError::Empty);
        }
        if value.len() > KEY_ID_LEN_MAX {
            return Err(PlaidKeyIdError::TooLong(value.len()));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaidKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PlaidKeyId({})", self.0)
    }
}

impl fmt::Display for PlaidKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
