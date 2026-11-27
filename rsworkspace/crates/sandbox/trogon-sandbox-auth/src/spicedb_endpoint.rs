use std::fmt;

use tonic::transport::Endpoint;

use crate::SpiceDbEndpointError;

/// The address of the permission service.
///
/// A bare `host:port` is read as `http://host:port`. Construction rejects anything that is not a
/// valid URI.
#[derive(Debug, Clone)]
pub struct SpiceDbEndpoint {
    uri: String,
    endpoint: Endpoint,
}

impl SpiceDbEndpoint {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SpiceDbEndpointError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(SpiceDbEndpointError::Empty);
        }
        let uri = if value.starts_with("http://") || value.starts_with("https://") {
            value.to_string()
        } else {
            format!("http://{value}")
        };
        let endpoint =
            Endpoint::from_shared(uri.clone()).map_err(|error| SpiceDbEndpointError::InvalidUri(error.to_string()))?;
        Ok(Self { uri, endpoint })
    }

    pub fn as_str(&self) -> &str {
        &self.uri
    }

    pub(crate) fn to_endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }
}

impl PartialEq for SpiceDbEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri
    }
}

impl Eq for SpiceDbEndpoint {}

impl fmt::Display for SpiceDbEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.uri)
    }
}
