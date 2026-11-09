use std::fmt;
use std::str::FromStr;
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

impl FromStr for SecretString {
    type Err = EmptySecretError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl<'de> serde::Deserialize<'de> for SecretString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

#[cfg(test)]
mod tests;
