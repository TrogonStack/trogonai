use serde::{Deserialize, Serialize};

use crate::constants::MAX_NATIVE_SANDBOX_ID_BYTES;
use crate::native_sandbox_id_error::NativeSandboxIdError;

/// The canonical identity E2B gives a sandbox: 1 to 128 ASCII letters, digits, `-` or `_`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct NativeSandboxId(String);

impl NativeSandboxId {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, NativeSandboxIdError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NATIVE_SANDBOX_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(NativeSandboxIdError);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NativeSandboxId {
    type Error = NativeSandboxIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NativeSandboxId> for String {
    fn from(value: NativeSandboxId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests;
