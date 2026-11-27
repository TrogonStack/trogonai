use std::str::FromStr;

use url::Url;

use crate::VercelEndpointError;

/// The base URL of the Vercel API.
///
/// Remote endpoints must use HTTPS; plain HTTP is accepted only for a loopback host. The URL carries no credentials,
/// query, or fragment.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VercelEndpoint(Url);

impl VercelEndpoint {
    pub fn new(url: Url) -> Result<Self, VercelEndpointError> {
        let secure = url.scheme() == "https"
            || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1" | "[::1]")));
        if !secure {
            return Err(VercelEndpointError::InsecureScheme);
        }
        let plain =
            url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none();
        if !plain {
            return Err(VercelEndpointError::NotPlain);
        }
        Ok(Self(url))
    }

    pub fn parse(value: &str) -> Result<Self, VercelEndpointError> {
        Self::new(Url::parse(value)?)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for VercelEndpoint {
    type Err = VercelEndpointError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
