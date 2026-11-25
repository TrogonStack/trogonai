use serde::{Deserialize, Serialize};

use crate::{CapabilityProfile, ProviderKind};

/// Request for tenant-facing capability profiles, optionally filtered to one provider.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListCapabilitiesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListCapabilitiesResponse {
    pub profiles: Vec<CapabilityProfile>,
}
