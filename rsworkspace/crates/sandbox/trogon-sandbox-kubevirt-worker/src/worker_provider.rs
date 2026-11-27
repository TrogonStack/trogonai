use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::ProviderCapabilityProfile;
use trogon_sandbox_protocol::{AuthorityHost, Provider};

use crate::{LeaseHolding, WarmPoolManager};

/// What [`crate::KubeVirtWorker`] needs from the provider it serves: the lease operations, the authority checks
/// on the leases it holds, and what its heartbeat reports.
#[async_trait]
pub trait WorkerProvider: Provider + AuthorityHost + LeaseHolding {
    /// What the worker advertises it can do.
    fn profile(&self) -> ProviderCapabilityProfile;

    /// The pool of pre-started VMs the worker keeps.
    fn warm_pool(&self) -> Arc<WarmPoolManager>;

    /// The most leases the worker accepts at once.
    fn max_concurrent_leases(&self) -> u32;

    /// How many leases the worker holds now.
    async fn current_leases(&self) -> u32;
}
