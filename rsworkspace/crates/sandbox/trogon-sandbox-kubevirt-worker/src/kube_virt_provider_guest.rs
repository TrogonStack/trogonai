use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use crate::{GuestLocator, KubeVirtProvider, LeaseHolding};

#[async_trait]
impl LeaseHolding for KubeVirtProvider {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease_id)
    }
}

#[async_trait]
impl GuestLocator for KubeVirtProvider {
    async fn vm_name(&self, lease_id: &LeaseId) -> Option<String> {
        self.active
            .lock()
            .await
            .get(lease_id)
            .map(|active| active.vm_name.clone())
    }

    async fn vmi_ip(&self, vm_name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.ops.vmi_ip(vm_name).await?)
    }
}
