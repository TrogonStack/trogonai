use url::Url;

use crate::FlyEndpointError;

/// The base URL of the Fly Machines API.
///
/// Remote endpoints must use HTTPS; plain HTTP is accepted only for a loopback host. The URL carries no
/// credentials, query, or fragment, and its own path is ignored.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FlyEndpoint(Url);

impl FlyEndpoint {
    pub fn new(url: Url) -> Result<Self, FlyEndpointError> {
        let secure = url.scheme() == "https"
            || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
        if !secure {
            return Err(FlyEndpointError::InsecureScheme);
        }
        let plain =
            url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none();
        if !plain {
            return Err(FlyEndpointError::NotPlain);
        }
        Ok(Self(url))
    }

    pub fn parse(value: &str) -> Result<Self, FlyEndpointError> {
        Self::new(Url::parse(value)?)
    }

    pub(crate) fn url(&self) -> &Url {
        &self.0
    }
}
