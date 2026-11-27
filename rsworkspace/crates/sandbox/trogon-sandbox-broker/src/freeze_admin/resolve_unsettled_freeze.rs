use trogon_sandbox_contract::{LeaseId, LeaseRecord};

use super::freeze_admin_error::FreezeAdminError;
use super::freeze_resolution::FreezeResolution;
use super::freeze_resolve_outcome::FreezeResolveOutcome;
use crate::active_lease::ActiveLeaseStore;
use crate::constants::FREEZE_RESOLVE_ATTEMPTS;
use crate::core::FREEZE_SETTLE_MAX_ATTEMPTS;
use crate::idempotency::IdempotencyStore;

pub(super) fn resolved(record: &LeaseRecord, resolution: FreezeResolution) -> LeaseRecord {
    let mut updated = record.clone();
    match (resolution, updated.freeze_in_flight.as_mut()) {
        (FreezeResolution::Clear, _) => updated.freeze_in_flight = None,
        (FreezeResolution::Retry, Some(marker)) => {
            marker.settle_attempts = 0;
            marker.next_settle_at = None;
        }
        (FreezeResolution::Retry, None) => {}
    }
    updated
}

/// Applies `resolution` to the lease's freeze marker. Refuses a marker that
/// has not used up its settle attempts unless `force`, because the reconciler
/// is still working it. A lease granted through a dispatch attempt keeps its
/// journal entry equal to its record, so the journal moves first, as the
/// broker's own updates do; a retry after a failure between the two writes
/// finds the journal already advanced and finishes the record.
pub async fn resolve_unsettled_freeze(
    active: &dyn ActiveLeaseStore,
    idempotency: &dyn IdempotencyStore,
    lease_id: &LeaseId,
    resolution: FreezeResolution,
    force: bool,
) -> Result<FreezeResolveOutcome, FreezeAdminError> {
    for _ in 0..FREEZE_RESOLVE_ATTEMPTS {
        let Some(record) = active.get(lease_id).await? else {
            return Ok(FreezeResolveOutcome::NoSuchLease);
        };
        let Some(marker) = record.freeze_in_flight else {
            return Ok(FreezeResolveOutcome::NoMarker);
        };
        if !force && marker.settle_attempts < FREEZE_SETTLE_MAX_ATTEMPTS {
            return Ok(FreezeResolveOutcome::NotUnsettled {
                settle_attempts: marker.settle_attempts,
            });
        }
        let updated = resolved(&record, resolution);
        if let Some(attempt) = &record.dispatch_attempt {
            let journaled = idempotency
                .get_attempt(&record.tenant, &attempt.idempotency_key)
                .await?;
            if journaled.as_ref() != Some(&updated)
                && !idempotency
                    .replace_attempt(&record.tenant, &attempt.idempotency_key, &record, &updated)
                    .await?
            {
                continue;
            }
        }
        if active.replace_if_unchanged(&record, &updated).await? {
            return Ok(FreezeResolveOutcome::Applied);
        }
    }
    Ok(FreezeResolveOutcome::Conflict)
}
