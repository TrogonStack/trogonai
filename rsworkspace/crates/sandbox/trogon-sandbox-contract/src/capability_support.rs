use serde::{Deserialize, Serialize};

/// Whether a worker offers one capability.
///
/// It serializes as a JSON boolean, `true` for `Supported`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CapabilitySupport {
    Supported,
    #[default]
    Unsupported,
}

impl CapabilitySupport {
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

impl From<bool> for CapabilitySupport {
    fn from(supported: bool) -> Self {
        if supported { Self::Supported } else { Self::Unsupported }
    }
}

impl From<CapabilitySupport> for bool {
    fn from(support: CapabilitySupport) -> Self {
        support.is_supported()
    }
}
