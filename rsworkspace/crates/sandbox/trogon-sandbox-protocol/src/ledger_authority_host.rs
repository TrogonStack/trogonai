use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{CancelIntent, ProviderCleanupRequest, ProviderCleanupResponse};

use crate::{AttemptLedger, AttemptPhase, AuthorityHost, AuthorityHostError, HeldLease, PurgeOutcome};

/// An [`AuthorityHost`] over the attempt ledger. Cleanup callbacks must bind the backend receipt and effect under
/// the provider's assignment gate. The guard has no owned freeze transition here.
pub struct LedgerAuthorityHost<F> {
    ledger: Arc<AttemptLedger>,
    cleanup: F,
}

impl<F> LedgerAuthorityHost<F> {
    /// A host that reads holdings from `ledger` and purges through `cleanup`.
    pub fn new(ledger: Arc<AttemptLedger>, cleanup: F) -> Self {
        Self { ledger, cleanup }
    }
}

#[async_trait]
impl<F, Fut> AuthorityHost for LedgerAuthorityHost<F>
where
    F: Fn(ProviderCleanupRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ProviderCleanupResponse, AuthorityHostError>> + Send,
{
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .ledger
            .try_states()
            .await?
            .into_iter()
            .filter(|state| state.allocation_started && !state.backend_cleaned)
            .map(|state| HeldLease::Targeted(state.identity))
            .collect())
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Targeted(identity) = lease else {
            return Ok(PurgeOutcome::Stale);
        };
        let Some(state) = self.ledger.authority_state(identity).await? else {
            return Ok(PurgeOutcome::Stale);
        };
        if state.phase == AttemptPhase::Cleaned && state.backend_cleaned && state.intent == CancelIntent::Purge {
            return Ok(PurgeOutcome::Settled);
        }
        let response = (self.cleanup)(ProviderCleanupRequest {
            identity: identity.clone(),
            intent: CancelIntent::Purge,
        })
        .await?;
        match response {
            ProviderCleanupResponse::Cleaned {
                identity: confirmed,
                intent: CancelIntent::Purge,
            } if confirmed == *identity => {
                let confirmed = self.ledger.authority_state(identity).await?;
                if confirmed.is_some_and(|state| {
                    state.phase == AttemptPhase::Cleaned && state.backend_cleaned && state.intent == CancelIntent::Purge
                }) {
                    Ok(PurgeOutcome::Purged)
                } else {
                    Err(AuthorityHostError::ConfirmationMissing)
                }
            }
            ProviderCleanupResponse::Failed {
                identity: failed,
                message,
            } if failed == *identity => Err(AuthorityHostError::CleanupFailed { message }),
            _ => Err(AuthorityHostError::CleanupReplyMismatch),
        }
    }
}
