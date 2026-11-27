use trogon_sandbox_contract::{CancelOwnedLeaseRequest, CancelOwnedLeaseResponse, LeaseError, LeaseId};

use super::constants::{CLEANUP_CLAIM_ROUND_TRIPS, CLEANUP_CLAIM_SLACK};
use super::{E2bProviderError, E2bWorkerProvider, LeaseAccess};
use crate::api::ApiError;
use crate::lease_cleanup::StoredCleanupReceipt;

impl E2bWorkerProvider {
    /// Drops the lease record when it still names exactly `sandbox_id`, and releases what hangs off it.
    pub(super) async fn forget_exact(&self, lease_id: &LeaseId, sandbox_id: &str) {
        let removed = {
            let mut active = self.active.lock().await;
            if active
                .get(lease_id.as_str())
                .is_some_and(|lease| lease.sandbox_id == sandbox_id)
            {
                active.remove(lease_id.as_str())
            } else {
                None
            }
        };
        if let Some(removed) = removed {
            self.exec.release_watches(lease_id.as_str(), &removed.target).await;
            self.exec.forget_lease(lease_id.as_str()).await;
        }
    }

    pub(super) async fn delete_sandbox_confirmed(&self, sandbox_id: &str) -> Result<(), E2bProviderError> {
        self.api.delete_sandbox(sandbox_id).await?;
        match self.api.get_sandbox(sandbox_id).await {
            Err(ApiError::NotFound(_)) => Ok(()),
            Ok(_) => Err(E2bProviderError::DeleteUnconfirmed),
            Err(error) => Err(error.into()),
        }
    }

    /// Cleans up the sandbox a signed owned-cleanup request names. `None` means this worker holds no authority
    /// over it.
    pub(crate) async fn cancel_owned(&self, request: &CancelOwnedLeaseRequest) -> Option<CancelOwnedLeaseResponse> {
        let gate = self.lease_gate(&request.lease_id);
        let _held = gate.lock().await;
        let has_active_authority = {
            let mut active = self.active.lock().await;
            match active.get_mut(request.lease_id.as_str()) {
                Some(held) => {
                    if held.tenant != request.tenant || held.sandbox_id != request.sandbox_id.as_str() {
                        return Some(CancelOwnedLeaseResponse::Denied(LeaseError::InvalidSpec {
                            reason: "owned cleanup tenant or sandbox identity does not match the held lease".into(),
                        }));
                    }
                    held.access = LeaseAccess::CleanupOnly;
                    true
                }
                None => false,
            }
        };
        let stored = if has_active_authority {
            match self.cleanup_receipts.begin(request).await {
                Ok(stored) => stored,
                Err(error) => {
                    return Some(CancelOwnedLeaseResponse::Denied(LeaseError::Backend {
                        message: error.to_string(),
                    }));
                }
            }
        } else {
            match self.cleanup_receipts.load(request).await {
                Ok(Some(stored)) => stored,
                Ok(None) => return None,
                Err(error) => {
                    tracing::warn!(%error, "owned cleanup receipt could not establish authority");
                    return None;
                }
            }
        };
        let result = if stored.absent() {
            Ok(())
        } else {
            self.cancel_journaled_resource(request, stored).await
        };
        Some(match result {
            Ok(()) => {
                self.forget_exact(&request.lease_id, request.sandbox_id.as_str()).await;
                CancelOwnedLeaseResponse::Cleaned {
                    tenant: request.tenant.clone(),
                    lease_id: request.lease_id.clone(),
                    sandbox_id: request.sandbox_id.clone(),
                }
            }
            Err(error) => CancelOwnedLeaseResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }),
        })
    }

    pub(super) async fn cancel_journaled_resource(
        &self,
        request: &CancelOwnedLeaseRequest,
        stored: StoredCleanupReceipt,
    ) -> Result<(), E2bProviderError> {
        let claimed = self
            .cleanup_receipts
            .claim(
                request,
                stored,
                self.api
                    .timeout()
                    .saturating_mul(CLEANUP_CLAIM_ROUND_TRIPS)
                    .saturating_add(CLEANUP_CLAIM_SLACK),
            )
            .await?;
        if let Err(error) = self.delete_sandbox_confirmed(request.sandbox_id.as_str()).await {
            return match self.cleanup_receipts.release(request, claimed).await {
                Ok(()) => Err(error),
                Err(journal) => Err(E2bProviderError::ClaimUnresolved {
                    source: Box::new(error),
                    journal,
                }),
            };
        }
        self.cleanup_receipts.confirm_absent(request, claimed).await?;
        Ok(())
    }
}
