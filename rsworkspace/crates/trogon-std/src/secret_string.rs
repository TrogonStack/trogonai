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
mod tests;
