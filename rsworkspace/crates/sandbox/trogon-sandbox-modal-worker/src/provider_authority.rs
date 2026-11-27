use async_trait::async_trait;
use trogon_sandbox_contract::{ProviderKind, SandboxId};
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, AuthorityResourceId, HeldLease, PurgeOutcome};

use crate::{ModalApiError, ModalProvider, ModalProviderError};

fn failed(error: impl Into<ModalProviderError>) -> AuthorityHostError {
    AuthorityHostError::Failed(Box::new(error.into()))
}

#[async_trait]
impl AuthorityHost for ModalProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.active
            .lock()
            .await
            .iter()
            .map(|(lease_id, held)| {
                Ok(HeldLease::Provider {
                    lease_id: lease_id.clone(),
                    provider: ProviderKind::Modal,
                    resource_id: AuthorityResourceId::new(held.sandbox_id.as_str()).map_err(failed)?,
                })
            })
            .collect()
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Modal,
            resource_id,
        } = lease
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let resource = SandboxId::new(resource_id.as_str()).map_err(failed)?;
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let exact = self
            .active
            .lock()
            .await
            .get(lease_id)
            .is_some_and(|held| held.sandbox_id == resource);
        if !exact {
            return match self.api.get_sandbox(&resource).await {
                Err(ModalApiError::SandboxNotFound(_)) => Ok(PurgeOutcome::Settled),
                Ok(_) => Ok(PurgeOutcome::Stale),
                Err(error) => Err(failed(error)),
            };
        }
        let outcome = match self.api.terminate(&resource).await {
            Ok(()) => PurgeOutcome::Purged,
            Err(ModalApiError::SandboxNotFound(_)) => PurgeOutcome::Settled,
            Err(error) => return Err(failed(error)),
        };
        self.active.lock().await.remove(lease_id);
        Ok(outcome)
    }
}
