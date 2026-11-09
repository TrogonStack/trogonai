use std::fmt;

use super::constants::ITEM_ID_LEN_MAX;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaidItemIdError {
    #[error("item_id must not be empty")]
    Empty,
    #[error("item_id contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("item_id is too long: {0} bytes (max {max})", max = ITEM_ID_LEN_MAX)]
    TooLong(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaidItemId(String);

impl PlaidItemId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, PlaidItemIdError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(PlaidItemIdError::Empty);
        }
        if value.len() > ITEM_ID_LEN_MAX {
            return Err(PlaidItemIdError::TooLong(value.len()));
        }
        if let Some(ch) = value.chars().find(|ch| !ch.is_ascii_graphic()) {
            return Err(PlaidItemIdError::InvalidCharacter(ch));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlaidItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
