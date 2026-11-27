use async_trait::async_trait;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

use crate::{AcaWorkerProvider, SessionDeleteOutcome};

#[async_trait]
impl AuthorityHost for AcaWorkerProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .active
            .lock()
            .await
            .iter()
            .filter(|(lease, held)| !self.scope.is_completed(lease, &held.resource))
            .map(|(lease_id, held)| HeldLease::Provider {
                lease_id: lease_id.clone(),
                provider: ProviderKind::AzureContainerApps,
                resource_id: held.resource.clone(),
            })
            .collect())
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::AzureContainerApps,
            resource_id,
        } = lease
        else {
            return Ok(FreezeOutcome::Stale);
        };
        Ok(if self.holds(lease_id, resource_id).await {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        })
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::AzureContainerApps,
            resource_id,
        } = lease
        else {
            return Ok(ThawOutcome::Stale);
        };
        Ok(
            if self.scope.is_completed(lease_id, resource_id) || self.holds(lease_id, resource_id).await {
                ThawOutcome::NoLongerOwned
            } else {
                ThawOutcome::Stale
            },
        )
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::AzureContainerApps,
            resource_id,
        } = lease
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.scope.is_completed(lease_id, resource_id) {
            return Ok(PurgeOutcome::Settled);
        }
        if !self.holds(lease_id, resource_id).await {
            return Ok(PurgeOutcome::Stale);
        }
        let outcome = self
            .cleanup_locked(lease_id)
            .await
            .map_err(|error| AuthorityHostError::Failed(Box::new(error)))?;
        Ok(if outcome == SessionDeleteOutcome::Deleted {
            PurgeOutcome::Purged
        } else {
            PurgeOutcome::Settled
        })
    }
}
