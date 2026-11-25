use crate::RegistrationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationId(String);

impl ApplicationId {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        if value.is_empty()
            || value.len() > 256
            || !value.as_bytes()[0].is_ascii_alphanumeric()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
        {
            return Err(RegistrationError::Binding);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
