use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::ProviderCapabilityProfile;

use crate::{KubeVirtProvider, WarmPoolManager, WorkerProvider};

#[async_trait]
impl WorkerProvider for KubeVirtProvider {
    fn profile(&self) -> ProviderCapabilityProfile {
        self.profile.clone()
    }

    fn warm_pool(&self) -> Arc<WarmPoolManager> {
        self.warm_pool.clone()
    }

    fn max_concurrent_leases(&self) -> u32 {
        self.max_concurrent_leases
    }

    async fn current_leases(&self) -> u32 {
        u32::try_from(self.active.lock().await.len()).unwrap_or(u32::MAX)
    }
}
