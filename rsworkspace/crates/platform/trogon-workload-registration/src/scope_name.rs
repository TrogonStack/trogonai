use crate::RegistrationError;
use crate::constants::MAX_SCOPE_NAME_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeName(String);

impl ScopeName {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        if value.is_empty()
            || value.len() > MAX_SCOPE_NAME_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        {
            return Err(RegistrationError::ScopeName);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
