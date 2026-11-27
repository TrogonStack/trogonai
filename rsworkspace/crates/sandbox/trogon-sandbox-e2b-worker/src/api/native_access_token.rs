use reqwest::header::HeaderValue;
use serde::Deserialize;

use super::NativeAccessTokenError;

/// An envd access token read back from E2B. Never shown in `Debug`.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct NativeAccessToken(String);

impl NativeAccessToken {
    pub(crate) fn into_value(self) -> Result<String, NativeAccessTokenError> {
        if self.0.trim().is_empty() || HeaderValue::from_str(&self.0).is_err() {
            return Err(NativeAccessTokenError);
        }
        Ok(self.0)
    }
}

impl std::fmt::Debug for NativeAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}
