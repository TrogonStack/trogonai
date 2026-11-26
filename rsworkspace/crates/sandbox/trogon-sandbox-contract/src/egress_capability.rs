use serde::{Deserialize, Serialize};

use crate::CapabilitySupport;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EgressCapability {
    #[serde(default)]
    pub deny_all: CapabilitySupport,
    #[serde(default)]
    pub host_allowlist: CapabilitySupport,
    #[serde(default)]
    pub cidr_allowlist: CapabilitySupport,
    #[serde(default)]
    pub proxy_mediated: CapabilitySupport,
}

impl EgressCapability {
    pub const NONE: Self = Self {
        deny_all: CapabilitySupport::Unsupported,
        host_allowlist: CapabilitySupport::Unsupported,
        cidr_allowlist: CapabilitySupport::Unsupported,
        proxy_mediated: CapabilitySupport::Unsupported,
    };

    pub const DENY_ONLY: Self = Self {
        deny_all: CapabilitySupport::Supported,
        ..Self::NONE
    };

    pub const PROXIED: Self = Self {
        deny_all: CapabilitySupport::Supported,
        host_allowlist: CapabilitySupport::Supported,
        cidr_allowlist: CapabilitySupport::Supported,
        proxy_mediated: CapabilitySupport::Supported,
    };
}
