use reqwest::header::HeaderValue;

use crate::FlyApiTokenError;

/// A Fly API token, held only as a sensitive `Authorization` header value.
#[derive(Clone)]
pub struct FlyApiToken(HeaderValue);

impl FlyApiToken {
    pub fn new(token: &str) -> Result<Self, FlyApiTokenError> {
        if token.is_empty() {
            return Err(FlyApiTokenError::Empty);
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| FlyApiTokenError::NotAHeader)?;
        header.set_sensitive(true);
        Ok(Self(header))
    }

    pub(crate) fn header(&self) -> HeaderValue {
        self.0.clone()
    }
}

impl std::fmt::Debug for FlyApiToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FlyApiToken(..)")
    }
}
