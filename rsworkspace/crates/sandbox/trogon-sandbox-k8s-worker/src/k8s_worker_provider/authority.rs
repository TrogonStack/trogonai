use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::{AuthorityResourceId, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};
use trogon_sandbox_providers::K8sResourceReceipt;

use crate::{K8sWorkerError, K8sWorkerProvider};

#[expect(
    clippy::result_large_err,
    reason = "K8sWorkerError carries the provider errors by value so callers can match on them"
)]
fn resource(lease: &LeaseId, receipt: &K8sResourceReceipt) -> Result<HeldLease, K8sWorkerError> {
    Ok(HeldLease::Provider {
        lease_id: lease.clone(),
        provider: ProviderKind::K8sMicroVm,
        resource_id: AuthorityResourceId::new(receipt.authority_key())?,
    })
}

fn expected_resource(expected: &HeldLease) -> Option<(&LeaseId, &str)> {
    match expected {
        HeldLease::Provider {
            lease_id,
            provider: ProviderKind::K8sMicroVm,
            resource_id,
        } => Some((lease_id, resource_id.as_str())),
        _ => None,
    }
}

impl K8sWorkerProvider {
    pub async fn authority_held_leases(&self) -> Result<Vec<HeldLease>, K8sWorkerError> {
        let mut held = Vec::new();
        for (lease, active) in self.active.lock().await.iter() {
            held.push(resource(&lease.clone(), &active.owner)?);
        }
        for (lease, resources) in self.cleanup.lock().await.iter() {
            if let Some(owner) = resources.first() {
                let lease = resource(&lease.clone(), owner)?;
                if !held.contains(&lease) {
                    held.push(lease);
                }
            }
        }
        Ok(held)
    }

    pub async fn authority_freeze(&self, expected: &HeldLease) -> Result<FreezeOutcome, K8sWorkerError> {
        let Some((lease, key)) = expected_resource(expected) else {
            return Ok(FreezeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease).lock_owned().await;
        let matches = self
            .active
            .lock()
            .await
            .get(lease)
            .is_some_and(|record| record.owner.authority_key() == key)
            || self
                .cleanup
                .lock()
                .await
                .get(lease)
                .and_then(|resources| resources.first())
                .is_some_and(|receipt| receipt.authority_key() == key);
        Ok(if matches {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        })
    }

    pub async fn authority_thaw(&self, expected: &HeldLease) -> Result<ThawOutcome, K8sWorkerError> {
        Ok(match self.authority_freeze(expected).await? {
            FreezeOutcome::Stale => ThawOutcome::Stale,
            _ => ThawOutcome::NoLongerOwned,
        })
    }

    pub async fn authority_purge<F, Fut>(&self, expected: &HeldLease, finish: F) -> Result<PurgeOutcome, K8sWorkerError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let Some((lease, key)) = expected_resource(expected) else {
            return Ok(PurgeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease).lock_owned().await;
        let active = self
            .active
            .lock()
            .await
            .get(lease)
            .cloned()
            .filter(|active| active.owner.authority_key() == key);
        let pending = self
            .cleanup
            .lock()
            .await
            .get(lease)
            .cloned()
            .filter(|resources| resources.first().is_some_and(|receipt| receipt.authority_key() == key));
        if active.is_none() && pending.is_none() {
            return Ok(PurgeOutcome::Stale);
        }
        if let Some(active) = active {
            self.release(&active, None).await?;
            self.active.lock().await.remove(lease);
        }
        if let Some(pending) = pending {
            self.inner.release_resources(&pending).await?;
            self.cleanup.lock().await.remove(lease);
        }
        if !self.active.lock().await.contains_key(lease) && !self.cleanup.lock().await.contains_key(lease) {
            self.forget_lease_state(lease).await;
            finish().await;
        }
        Ok(PurgeOutcome::Purged)
    }
}
