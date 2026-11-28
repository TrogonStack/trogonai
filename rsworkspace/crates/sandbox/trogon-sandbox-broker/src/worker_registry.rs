use async_trait::async_trait;
use trogon_sandbox_contract::{
    CapabilityRequirements, Digest, ProviderCapabilityProfile, ProviderKind, WorkerHeartbeat, WorkerId,
};

#[async_trait]
pub trait WorkerRegistry: Send + Sync {
    async fn current_worker(
        &self,
        _provider: ProviderKind,
        _worker: &trogon_sandbox_contract::WorkerId,
    ) -> Option<WorkerHeartbeat> {
        None
    }

    async fn available_for(&self, provider: ProviderKind) -> Vec<WorkerId>;
    async fn any_available(&self, provider: ProviderKind) -> bool;
    async fn eligible_for(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
    ) -> Vec<WorkerHeartbeat>;
    async fn any_eligible(&self, provider: ProviderKind, requirements: Option<&CapabilityRequirements>) -> bool;
    async fn any_eligible_with_cached_digest(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
        digest: &Digest,
    ) -> bool;
    async fn any_can_restore(&self, provider: ProviderKind, requirements: Option<&CapabilityRequirements>) -> bool;
    async fn profile_for_provider(&self, provider: ProviderKind) -> Option<ProviderCapabilityProfile>;
    async fn warm_pool_count(&self, provider: ProviderKind, digest: &Digest) -> u32;

    /// Returns the freshest capability profile for every provider that
    /// currently has at least one non-stale heartbeat, one entry per
    /// distinct [`ProviderKind`].
    async fn list_capability_profiles(&self) -> Vec<ProviderCapabilityProfile>;
}

mod heartbeat_selection;
mod in_memory_worker_registry;
mod nats_worker_registry;
mod prefers_targeted_dispatch;
mod provider_selection;
mod provider_selection_error;
mod requires_targeted_dispatch;
mod selection;
mod worker_selection_constraints;

pub use in_memory_worker_registry::InMemoryWorkerRegistry;
pub use nats_worker_registry::NatsWorkerRegistry;
pub use prefers_targeted_dispatch::prefers_targeted_dispatch;
pub use provider_selection::ProviderSelection;
pub use provider_selection_error::ProviderSelectionError;
pub use requires_targeted_dispatch::requires_targeted_dispatch;
pub use selection::{
    lacks_snapshot_persistence, select_provider, select_provider_for_native_volumes, select_provider_in_regions,
    select_provider_with_constraints,
};
pub use worker_selection_constraints::WorkerSelectionConstraints;

#[cfg(test)]
mod tests;
