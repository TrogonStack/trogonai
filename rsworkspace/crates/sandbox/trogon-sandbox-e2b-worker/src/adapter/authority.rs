use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome,
};

use super::constants::{STATE_PAUSED, STATE_RUNNING};
use super::{E2bAuthorityError, E2bWorkerProvider, LeaseAccess, PauseOwnership};
use crate::api::ApiError;

fn native_held(lease: &HeldLease) -> Option<(&LeaseId, &str)> {
    match lease {
        HeldLease::Provider {
            lease_id,
            provider: ProviderKind::E2b,
            resource_id,
        } => Some((lease_id, resource_id.as_str())),
        _ => None,
    }
}

impl E2bWorkerProvider {
    async fn set_pause(&self, lease_id: &LeaseId, pause: PauseOwnership) {
        if let Some(held) = self.active.lock().await.get_mut(lease_id.as_str()) {
            held.pause = pause;
        }
    }

    async fn held_leases_inner(&self) -> Result<Vec<HeldLease>, E2bAuthorityError> {
        self.active
            .lock()
            .await
            .iter()
            .map(|(lease, held)| {
                Ok(HeldLease::Provider {
                    lease_id: LeaseId::new(lease.clone()),
                    provider: ProviderKind::E2b,
                    resource_id: AuthorityResourceId::new(held.sandbox_id.clone())?,
                })
            })
            .collect()
    }

    async fn freeze_inner(&self, lease: &HeldLease) -> Result<FreezeOutcome, E2bAuthorityError> {
        let Some((lease_id, sandbox_id)) = native_held(lease) else {
            return Ok(FreezeOutcome::Stale);
        };
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let ownership = match self.active.lock().await.get(lease_id.as_str()) {
            Some(held) if held.sandbox_id == sandbox_id && held.access == LeaseAccess::Ready => held.pause,
            Some(held) if held.sandbox_id == sandbox_id => return Err(E2bAuthorityError::CleanupOnly),
            _ => return Ok(FreezeOutcome::Stale),
        };
        let state = match self.api.get_sandbox(sandbox_id).await {
            Ok(detail) => detail.state,
            Err(ApiError::NotFound(_)) => return Ok(FreezeOutcome::Stale),
            Err(error) => return Err(error.into()),
        };
        if state == STATE_PAUSED {
            return Ok(if ownership.is_authority() {
                FreezeOutcome::Frozen
            } else {
                FreezeOutcome::AlreadyPaused
            });
        }
        if state != STATE_RUNNING {
            return Err(E2bAuthorityError::StateUnconfirmed);
        }
        if ownership != PauseOwnership::Manual {
            self.set_pause(lease_id, PauseOwnership::AuthorityPending).await;
        }
        match self.api.pause_sandbox(sandbox_id).await {
            Ok(()) => {
                if ownership == PauseOwnership::Manual {
                    return Ok(FreezeOutcome::AlreadyPaused);
                }
                self.set_pause(lease_id, PauseOwnership::Authority).await;
                Ok(FreezeOutcome::Frozen)
            }
            Err(ApiError::Conflict(_)) => {
                let restored = if ownership == PauseOwnership::Manual {
                    PauseOwnership::Manual
                } else {
                    PauseOwnership::Unowned
                };
                self.set_pause(lease_id, restored).await;
                match self.api.get_sandbox(sandbox_id).await {
                    Ok(detail) if detail.state == STATE_PAUSED => Ok(FreezeOutcome::AlreadyPaused),
                    _ => Err(E2bAuthorityError::PauseConflictUnconfirmed),
                }
            }
            Err(error) => {
                if ownership != PauseOwnership::Manual
                    && !matches!(&error, ApiError::Transport(_) | ApiError::Unavailable { .. })
                {
                    self.set_pause(lease_id, PauseOwnership::Unowned).await;
                }
                Err(error.into())
            }
        }
    }

    async fn thaw_inner(&self, lease: &HeldLease) -> Result<ThawOutcome, E2bAuthorityError> {
        let Some((lease_id, sandbox_id)) = native_held(lease) else {
            return Ok(ThawOutcome::Stale);
        };
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let state = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .filter(|held| held.sandbox_id == sandbox_id && held.access == LeaseAccess::Ready)
            .map(|held| (held.pause, held.lifetime));
        let Some((ownership, lifetime)) = state else {
            return match self.api.get_sandbox(sandbox_id).await {
                Err(ApiError::NotFound(_)) => Ok(ThawOutcome::NoLongerOwned),
                Ok(_) => Ok(ThawOutcome::Stale),
                Err(error) => Err(error.into()),
            };
        };
        if !ownership.is_authority() {
            return Ok(ThawOutcome::NoLongerOwned);
        }
        match self.resume_lease(lease_id, sandbox_id, lifetime).await {
            Ok(()) => {
                self.set_pause(lease_id, PauseOwnership::Unowned).await;
                Ok(ThawOutcome::Thawed)
            }
            Err(ApiError::NotFound(_)) => {
                self.forget_exact(lease_id, sandbox_id).await;
                Ok(ThawOutcome::NoLongerOwned)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn purge_inner(&self, lease: &HeldLease) -> Result<PurgeOutcome, E2bAuthorityError> {
        let Some((lease_id, sandbox_id)) = native_held(lease) else {
            return Ok(PurgeOutcome::Stale);
        };
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let exact = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| held.sandbox_id == sandbox_id);
        if !exact {
            return match self.api.get_sandbox(sandbox_id).await {
                Err(ApiError::NotFound(_)) => Ok(PurgeOutcome::Settled),
                Ok(_) => Ok(PurgeOutcome::Stale),
                Err(error) => Err(error.into()),
            };
        }
        let removed = self.api.delete_sandbox(sandbox_id).await?;
        self.forget_exact(lease_id, sandbox_id).await;
        Ok(if removed {
            PurgeOutcome::Purged
        } else {
            PurgeOutcome::Settled
        })
    }
}

#[async_trait]
impl AuthorityHost for E2bWorkerProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self.held_leases_inner().await?)
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        Ok(self.freeze_inner(lease).await?)
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        Ok(self.thaw_inner(lease).await?)
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        Ok(self.purge_inner(lease).await?)
    }
}
