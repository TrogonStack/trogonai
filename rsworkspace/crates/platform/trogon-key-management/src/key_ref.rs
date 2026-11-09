#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyRef(String);

impl KeyRef {
    pub fn new(value: impl Into<String>) -> Result<Self, KeyRefError> {
        let value = value.into();
        if value.is_empty() {
            return Err(KeyRefError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid key ref")]
pub struct KeyRefError;
