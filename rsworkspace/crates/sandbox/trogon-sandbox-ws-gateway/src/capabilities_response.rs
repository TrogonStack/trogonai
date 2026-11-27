use serde::Serialize;

use crate::ProviderCapabilityEntry;

/// Every provider and its live capability, as the `list_capabilities` tool returns it.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResponse {
    pub providers: Vec<ProviderCapabilityEntry>,
}
