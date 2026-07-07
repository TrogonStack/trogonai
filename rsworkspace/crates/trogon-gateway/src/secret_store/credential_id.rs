use std::fmt;
use std::sync::Arc;

const MAX_CREDENTIAL_ID_LEN: usize = 256;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialId(Arc<str>);

impl CredentialId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialIdError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(CredentialIdError::Empty);
        }
        if char_count > MAX_CREDENTIAL_ID_LEN {
            return Err(CredentialIdError::TooLong(char_count));
        }
        for ch in value.chars() {
            if !is_credential_id_char(ch) {
                return Err(CredentialIdError::InvalidCharacter(ch));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialId").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialIdError {
    #[error("credential id must not be empty")]
    Empty,
    #[error("credential id contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("credential id exceeds maximum length: {0}")]
    TooLong(usize),
}

fn is_credential_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_storage_safe_id() {
        let id = CredentialId::new("static-config:github/primary:webhook_secret").unwrap();

        assert_eq!(id.as_str(), "static-config:github/primary:webhook_secret");
    }

    #[test]
    fn rejects_empty_id() {
        assert_eq!(CredentialId::new(""), Err(CredentialIdError::Empty));
    }

    #[test]
    fn rejects_spaces() {
        assert_eq!(
            CredentialId::new("github primary"),
            Err(CredentialIdError::InvalidCharacter(' '))
        );
    }
}
