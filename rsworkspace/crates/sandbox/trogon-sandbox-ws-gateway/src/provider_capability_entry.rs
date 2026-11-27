use serde::Serialize;
use trogon_sandbox_contract::{IsolationTier, ProviderKind};

/// What one provider offers right now.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderCapabilityEntry {
    pub kind: ProviderKind,
    pub isolation_tiers: Vec<IsolationTier>,
    pub live_workers: u32,
    pub worker_capabilities: u32,
}
