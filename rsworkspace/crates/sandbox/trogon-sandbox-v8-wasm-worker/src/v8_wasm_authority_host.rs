use async_trait::async_trait;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

use crate::V8WasmProvider;
use crate::retire_outcome::RetireOutcome;

#[async_trait]
impl AuthorityHost for V8WasmProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .state
            .active
            .lock()
            .await
            .iter()
            .map(|(lease, held)| (trogon_sandbox_contract::LeaseId::new(lease.as_str()), held))
            .filter(|(lease, held)| !self.state.scope.is_completed(lease, &held.resource))
            .map(|(lease_id, held)| HeldLease::Provider {
                lease_id,
                provider: ProviderKind::V8Wasm,
                resource_id: held.resource.clone(),
            })
            .collect())
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::V8Wasm,
            resource_id,
        } = lease
        else {
            return Ok(FreezeOutcome::Stale);
        };
        let held = self
            .state
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| &held.resource == resource_id);
        Ok(if held {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        })
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::V8Wasm,
            resource_id,
        } = lease
        else {
            return Ok(ThawOutcome::Stale);
        };
        let held = self
            .state
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| &held.resource == resource_id);
        Ok(if held || self.state.scope.is_completed(lease_id, resource_id) {
            ThawOutcome::NoLongerOwned
        } else {
            ThawOutcome::Stale
        })
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::V8Wasm,
            resource_id,
        } = lease
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let retired = self.state.retire(lease_id, Some(resource_id)).await;
        let outcome = retired.map(purge_outcome);
        outcome
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
            .map_err(AuthorityHostError::Failed)
    }
}

fn purge_outcome(outcome: RetireOutcome) -> PurgeOutcome {
    match outcome {
        RetireOutcome::Purged => PurgeOutcome::Purged,
        RetireOutcome::Settled => PurgeOutcome::Settled,
        RetireOutcome::Stale => PurgeOutcome::Stale,
    }
}
