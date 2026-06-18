use super::lease_config_error::LeaseConfigError;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LeaseKey(String);

impl LeaseKey {
    pub fn new(key: impl Into<String>) -> Result<Self, LeaseConfigError> {
        let key = key.into();
        if key.is_empty() {
            return Err(LeaseConfigError::EmptyKey);
        }
        if !Self::is_valid_name(&key) {
            return Err(LeaseConfigError::InvalidKeyName(key));
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid_name(key: &str) -> bool {
        key.chars().all(Self::is_valid_char)
    }

    fn is_valid_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '/' || ch == '=' || ch == '.'
    }
}
