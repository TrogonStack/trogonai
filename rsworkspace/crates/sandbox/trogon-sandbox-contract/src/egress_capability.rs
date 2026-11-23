use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EgressCapability {
    #[serde(default)]
    pub deny_all: bool,
    #[serde(default)]
    pub host_allowlist: bool,
    #[serde(default)]
    pub cidr_allowlist: bool,
    #[serde(default)]
    pub proxy_mediated: bool,
}

impl EgressCapability {
    pub const NONE: Self = Self {
        deny_all: false,
        host_allowlist: false,
        cidr_allowlist: false,
        proxy_mediated: false,
    };

    pub const DENY_ONLY: Self = Self {
        deny_all: true,
        ..Self::NONE
    };

    pub const PROXIED: Self = Self {
        deny_all: true,
        host_allowlist: true,
        cidr_allowlist: true,
        proxy_mediated: true,
    };
}
