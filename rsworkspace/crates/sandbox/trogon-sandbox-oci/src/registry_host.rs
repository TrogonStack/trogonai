use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use crate::RegistryHostError;

/// A registry origin written as `host` or `host:port`, with no scheme, credentials, or path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegistryHost(String);

impl RegistryHost {
    pub fn new(value: impl Into<String>) -> Result<Self, RegistryHostError> {
        let value = value.into();
        let url = reqwest::Url::parse(&format!("https://{value}/")).map_err(|_| RegistryHostError::Malformed)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RegistryHostError::Credentials);
        }
        if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
            return Err(RegistryHostError::ExtraComponents);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn origin(&self, scheme: &str) -> Result<reqwest::Url, RegistryHostError> {
        reqwest::Url::parse(&format!("{scheme}://{}/", self.0)).map_err(|_| RegistryHostError::Malformed)
    }
}

impl FromStr for RegistryHost {
    type Err = RegistryHostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Borrow<str> for RegistryHost {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegistryHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
