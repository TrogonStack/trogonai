use crate::identifier::{validate_prefixed_identifier, IdentifierError};

/// Canonical Trogonai session identifier (`sess_...`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub const PREFIX: &'static str = "sess_";

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_prefixed_identifier(&value, Self::PREFIX)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for SessionId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&str> for SessionId {
    type Error = IdentifierError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_requires_prefix() {
        assert!(SessionId::new("sess_abc").is_ok());
        assert_eq!(
            SessionId::new("abc").err(),
            Some(IdentifierError::InvalidCharacter('_'))
        );
    }
}
