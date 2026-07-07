use std::fmt;
use std::sync::Arc;

const MAX_OWNER_ID_LEN: usize = 128;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialOwnerId(Arc<str>);

impl CredentialOwnerId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialOwnerIdError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(CredentialOwnerIdError::Empty);
        }
        if char_count > MAX_OWNER_ID_LEN {
            return Err(CredentialOwnerIdError::TooLong(char_count));
        }
        for ch in value.chars() {
            if !is_owner_id_char(ch) {
                return Err(CredentialOwnerIdError::InvalidCharacter(ch));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn static_config() -> Self {
        Self(Arc::from("static-config"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialOwnerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialOwnerId").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialOwnerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialOwnerIdError {
    #[error("credential owner id must not be empty")]
    Empty,
    #[error("credential owner id contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("credential owner id exceeds maximum length: {0}")]
    TooLong(usize),
}

fn is_owner_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_config_owner_is_stable() {
        assert_eq!(CredentialOwnerId::static_config().as_str(), "static-config");
    }

    #[test]
    fn rejects_empty_owner_id() {
        assert_eq!(CredentialOwnerId::new(""), Err(CredentialOwnerIdError::Empty));
    }
}
