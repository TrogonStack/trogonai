use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaidClientIdError {
    #[error("client_id must not be empty")]
    Empty,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PlaidClientId(String);

impl PlaidClientId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, PlaidClientIdError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(PlaidClientIdError::Empty);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaidClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PlaidClientId({})", self.0)
    }
}

impl fmt::Display for PlaidClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
