use trogon_sandbox_contract::{CapabilityRequirements, Digest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelectionError {
    NoProviderAvailable,
    NoWorkerSatisfiesCapabilities(CapabilityRequirements),
    NoWorkerCanRestoreSnapshot(Digest),
    /// Every region in the ordered list was tried and none had a worker, yet a
    /// worker outside those regions could have served the rest of the request.
    NoWorkerInRegion(Vec<String>),
}
