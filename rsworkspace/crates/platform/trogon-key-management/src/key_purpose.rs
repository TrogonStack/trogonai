#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyPurpose(String);

impl KeyPurpose {
    pub fn new(value: impl Into<String>) -> Result<Self, KeyPurposeError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 256 {
            return Err(KeyPurposeError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid key purpose")]
pub struct KeyPurposeError;
