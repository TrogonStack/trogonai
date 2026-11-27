use std::fmt;
use std::str::FromStr;

use serde::{Serialize, Serializer};
use url::Url;

use crate::WsGatewayUrlError;

/// A parsed `ws://` or `wss://` endpoint the browser connects to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsGatewayUrl(Url);

impl WsGatewayUrl {
    pub fn parse(input: &str) -> Result<Self, WsGatewayUrlError> {
        let url = Url::parse(input)?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(WsGatewayUrlError::UnsupportedScheme(url.scheme().to_owned()));
        }
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for WsGatewayUrl {
    type Err = WsGatewayUrlError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for WsGatewayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for WsGatewayUrl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
