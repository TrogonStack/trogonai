use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryBucket(String);

#[derive(Debug, thiserror::Error)]
#[error("registry bucket must contain 1 to 128 ASCII letters, digits, underscores or hyphens")]
pub struct RegistryBucketError;

impl RegistryBucket {
    pub fn new(value: String) -> Result<Self, RegistryBucketError> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(RegistryBucketError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegistryBucket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
