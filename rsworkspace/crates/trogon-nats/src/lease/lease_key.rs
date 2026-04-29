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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_alphanumeric_key() {
        let k = LeaseKey::new("my-key_v1").unwrap();
        assert_eq!(k.as_str(), "my-key_v1");
    }

    #[test]
    fn valid_key_with_all_allowed_special_chars() {
        assert!(LeaseKey::new("a/b=c.d-e_f").is_ok());
    }

    #[test]
    fn empty_key_is_rejected() {
        assert!(matches!(LeaseKey::new(""), Err(LeaseConfigError::EmptyKey)));
    }

    #[test]
    fn key_with_space_is_rejected() {
        assert!(matches!(LeaseKey::new("bad key"), Err(LeaseConfigError::InvalidKeyName(_))));
    }

    #[test]
    fn key_with_nats_wildcard_is_rejected() {
        assert!(matches!(LeaseKey::new("bad*key"), Err(LeaseConfigError::InvalidKeyName(_))));
    }

    #[test]
    fn key_with_at_sign_is_rejected() {
        assert!(matches!(LeaseKey::new("user@host"), Err(LeaseConfigError::InvalidKeyName(_))));
    }
}
