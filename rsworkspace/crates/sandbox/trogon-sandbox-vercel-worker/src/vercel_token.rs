use std::fmt;
use std::str::FromStr;

use reqwest::header::HeaderValue;

use crate::VercelTokenError;

/// A Vercel access token, held only as a sensitive `Authorization` header value so it never shows up in `Debug`
/// output or logs.
#[derive(Clone)]
pub struct VercelToken(HeaderValue);

impl VercelToken {
    pub fn new(token: &str) -> Result<Self, VercelTokenError> {
        if token.is_empty() {
            return Err(VercelTokenError::Empty);
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| VercelTokenError::NotAHeader)?;
        header.set_sensitive(true);
        Ok(Self(header))
    }

    pub(crate) fn header(&self) -> HeaderValue {
        self.0.clone()
    }
}

impl FromStr for VercelToken {
    type Err = VercelTokenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Debug for VercelToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VercelToken(..)")
    }
}
