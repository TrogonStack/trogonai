use std::fmt;
use std::str::FromStr;

use url::Url;

use crate::VolumeEndpointError;

/// An absolute `http` or `https` URL of an S3-compatible endpoint. Such URLs always name a host.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct VolumeEndpoint {
    text: String,
    plain_http: bool,
}

impl VolumeEndpoint {
    pub fn new(value: impl AsRef<str>) -> Result<Self, VolumeEndpointError> {
        let text = value.as_ref().trim();
        let url = Url::parse(text)?;
        let plain_http = match url.scheme() {
            "http" => true,
            "https" => false,
            other => return Err(VolumeEndpointError::UnsupportedScheme(other.to_owned())),
        };
        Ok(Self {
            text: text.to_owned(),
            plain_http,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the endpoint uses unencrypted `http`.
    pub fn is_plain_http(&self) -> bool {
        self.plain_http
    }
}

impl FromStr for VolumeEndpoint {
    type Err = VolumeEndpointError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for VolumeEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}
