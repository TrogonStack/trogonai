use super::NativeKvCasError;
use crate::constants::MAX_NATIVE_KV_KEY_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeKvKey(String);

impl NativeKvKey {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeKvCasError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NATIVE_KV_KEY_BYTES
            || value.starts_with('.')
            || value.ends_with('.')
            || value.contains("..")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_/=.".contains(&byte))
        {
            return Err(NativeKvCasError::InvalidKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
