use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum PlatformSupport {
    Unsupported,
    Supported,
}

impl From<bool> for PlatformSupport {
    fn from(value: bool) -> Self {
        if value { Self::Supported } else { Self::Unsupported }
    }
}

impl From<PlatformSupport> for bool {
    fn from(value: PlatformSupport) -> Self {
        matches!(value, PlatformSupport::Supported)
    }
}
