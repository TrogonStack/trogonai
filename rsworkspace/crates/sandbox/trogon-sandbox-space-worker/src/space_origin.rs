use url::Url;

use crate::SpaceOriginError;

/// A SPACE endpoint origin: HTTPS, or plain HTTP only to a loopback host, with no
/// credentials, query, fragment or path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SpaceOrigin(Url);

impl SpaceOrigin {
    /// Parses an origin from a URL.
    pub fn parse(url: Url) -> Result<Self, SpaceOriginError> {
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1" | "[::1]"));
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err(SpaceOriginError::InsecureRemote);
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(SpaceOriginError::NotAnOrigin);
        }
        Ok(Self(url))
    }

    /// The origin as a URL.
    pub fn url(&self) -> &Url {
        &self.0
    }

    /// Whether `url` has this origin's scheme, host and port and carries no
    /// credentials or fragment.
    pub fn admits(&self, url: &Url) -> bool {
        url.scheme() == self.0.scheme()
            && url.host_str() == self.0.host_str()
            && url.port_or_known_default() == self.0.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    }
}
