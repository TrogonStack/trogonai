use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome,
};

use crate::locked::locked;
use crate::{KubeVirtProvider, KubeVirtProviderError, LeaseResources};

fn failed(error: impl Into<KubeVirtProviderError>) -> AuthorityHostError {
    AuthorityHostError::Failed(Box::new(error.into()))
}

fn held(lease_id: &LeaseId, resources: &LeaseResources) -> Result<Option<HeldLease>, AuthorityHostError> {
    resources
        .authority_key()
        .map(|key| {
            Ok(HeldLease::Provider {
                lease_id: lease_id.clone(),
                provider: ProviderKind::KubeVirt,
                resource_id: AuthorityResourceId::new(key).map_err(failed)?,
            })
        })
        .transpose()
}

/// The lease and resource key of a KubeVirt assignment, or `None` for anything else.
fn expected(lease: &HeldLease) -> Option<(&LeaseId, &str)> {
    match lease {
        HeldLease::Provider {
            lease_id,
            provider: ProviderKind::KubeVirt,
            resource_id,
        } => Some((lease_id, resource_id.as_str())),
        _ => None,
    }
}

impl KubeVirtProvider {
    /// Every lease memory holds resources for, except allocations still preparing: those are not yet exact.
    async fn authority_candidates(&self) -> Vec<(LeaseId, LeaseResources)> {
        let mut candidates: Vec<(LeaseId, LeaseResources)> = Vec::new();
        let active: Vec<_> = self
            .active
            .lock()
            .await
            .iter()
            .map(|(lease, active)| (lease.clone(), active.resources.clone()))
            .collect();
        let retained: Vec<_> = self
            .retained
            .lock()
            .await
            .iter()
            .map(|(lease, resources)| (lease.clone(), resources.clone()))
            .collect();
        let in_flight: Vec<_> = locked(&self.in_flight)
            .iter()
            .map(|(lease, resources)| (lease.clone(), locked(resources).clone()))
            .collect();
        let preparing = locked(&self.preparing).clone();
        for (lease, resources) in active.into_iter().chain(retained).chain(in_flight) {
            let duplicate = candidates.iter().any(|(known, known_resources)| {
                *known == lease && known_resources.authority_key() == resources.authority_key()
            });
            if !duplicate {
                candidates.push((lease, resources));
            }
        }
        candidates.retain(|(lease, _)| !preparing.contains(lease));
        candidates
    }
}

#[async_trait]
impl AuthorityHost for KubeVirtProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        let mut result = Vec::new();
        for (lease, resources) in self.authority_candidates().await {
            if let Some(held) = held(&lease, &resources)?
                && !result.contains(&held)
            {
                result.push(held);
            }
        }
        Ok(result)
    }

    /// KubeVirt cannot pause a guest without a cold stop, which would lose its memory, so a held lease reports
    /// [`FreezeOutcome::Unsupported`] and an unknown one [`FreezeOutcome::Stale`].
    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        let Some((lease_id, key)) = expected(lease) else {
            return Ok(FreezeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        Ok(if self.owns(lease_id, key).await {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        })
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        Ok(match self.freeze(lease).await? {
            FreezeOutcome::Stale => ThawOutcome::Stale,
            _ => ThawOutcome::NoLongerOwned,
        })
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let Some((lease_id, key)) = expected(lease) else {
            return Ok(PurgeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        if !self.owns(lease_id, key).await {
            return Ok(PurgeOutcome::Stale);
        }
        self.cancel_locked(lease_id).await.map_err(failed)?;
        Ok(PurgeOutcome::Purged)
    }
}
