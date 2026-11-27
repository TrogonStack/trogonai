use std::fmt;
use std::str::FromStr;

use reqwest::header::HeaderValue;

use crate::DaytonaApiKeyError;

/// A Daytona API key, held only as a sensitive `Authorization` header value so it never shows up in `Debug` output
/// or logs.
#[derive(Clone)]
pub struct DaytonaApiKey(HeaderValue);

impl DaytonaApiKey {
    pub fn new(key: &str) -> Result<Self, DaytonaApiKeyError> {
        if key.is_empty() {
            return Err(DaytonaApiKeyError::Empty);
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| DaytonaApiKeyError::NotAHeader)?;
        header.set_sensitive(true);
        Ok(Self(header))
    }

    pub(crate) fn header(&self) -> HeaderValue {
        self.0.clone()
    }
}

impl FromStr for DaytonaApiKey {
    type Err = DaytonaApiKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Debug for DaytonaApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DaytonaApiKey(..)")
    }
}
