use reqwest::Url;

use super::constants::WEBHOOK_VERIFICATION_KEY_PATH;

#[derive(Debug, thiserror::Error)]
pub enum PlaidApiBaseUrlError {
    #[error("invalid Plaid API base URL: {0}")]
    Parse(#[source] url::ParseError),
    #[error("invalid Plaid API base URL: must use http or https")]
    UnsupportedScheme,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaidApiBaseUrl {
    webhook_verification_key: Url,
}

impl PlaidApiBaseUrl {
    pub fn new(s: impl AsRef<str>) -> Result<Self, PlaidApiBaseUrlError> {
        let mut base = Url::parse(s.as_ref()).map_err(PlaidApiBaseUrlError::Parse)?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(PlaidApiBaseUrlError::UnsupportedScheme);
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let webhook_verification_key = base
            .join(WEBHOOK_VERIFICATION_KEY_PATH)
            .map_err(PlaidApiBaseUrlError::Parse)?;
        Ok(Self {
            webhook_verification_key,
        })
    }

    pub fn webhook_verification_key_url(&self) -> &Url {
        &self.webhook_verification_key
    }
}

#[cfg(test)]
mod tests;
