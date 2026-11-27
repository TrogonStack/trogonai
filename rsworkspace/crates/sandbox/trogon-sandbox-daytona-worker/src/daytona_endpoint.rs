use std::str::FromStr;

use url::Url;

use crate::DaytonaEndpointError;

/// The base URL of the Daytona API.
///
/// Remote endpoints must use HTTPS; plain HTTP is accepted only for a loopback host. The URL carries no credentials,
/// query, or fragment.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DaytonaEndpoint(Url);

impl DaytonaEndpoint {
    pub fn new(url: Url) -> Result<Self, DaytonaEndpointError> {
        let secure = url.scheme() == "https"
            || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
        if !secure {
            return Err(DaytonaEndpointError::InsecureScheme);
        }
        let plain =
            url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none();
        if !plain {
            return Err(DaytonaEndpointError::NotPlain);
        }
        Ok(Self(url))
    }

    pub fn parse(value: &str) -> Result<Self, DaytonaEndpointError> {
        Self::new(Url::parse(value)?)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for DaytonaEndpoint {
    type Err = DaytonaEndpointError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
