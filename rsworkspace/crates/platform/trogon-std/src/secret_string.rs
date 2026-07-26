use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
pub struct SecretString(Arc<str>);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("secret must not be empty")]
pub struct EmptySecretError;

impl SecretString {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(EmptySecretError);
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
mod tests;
