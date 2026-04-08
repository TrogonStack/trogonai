use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
pub struct SecretString(Arc<str>);

#[derive(Debug, PartialEq, Eq)]
pub struct EmptySecret;

impl fmt::Display for EmptySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("secret must not be empty")
    }
}

impl std::error::Error for EmptySecret {}

impl SecretString {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(EmptySecret);
        }
        Ok(Self(Arc::from(s)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_secret() {
        let secret = SecretString::new("my-secret").unwrap();
        assert_eq!(secret.as_str(), "my-secret");
    }

    #[test]
    fn empty_secret_rejected() {
        assert!(matches!(SecretString::new(""), Err(EmptySecret)));
    }

    #[test]
    fn debug_redacts_value() {
        let secret = SecretString::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
    }

    #[test]
    fn clone_shares_arc() {
        let a = SecretString::new("secret").unwrap();
        let b = a.clone();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn error_display() {
        assert_eq!(EmptySecret.to_string(), "secret must not be empty");
    }
}
