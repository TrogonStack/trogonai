use trogon_sandbox_contract::{ProviderKind, WorkerId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSelection {
    pub provider: ProviderKind,
    pub locality_hit: bool,
    pub worker_id: Option<WorkerId>,
}
