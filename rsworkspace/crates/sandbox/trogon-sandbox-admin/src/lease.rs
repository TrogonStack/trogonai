use trogon_sandbox_broker::active_lease::ActiveLeaseStore;
use trogon_sandbox_broker::freeze_admin::{FreezeResolution, FreezeResolveOutcome, resolve_unsettled_freeze};
use trogon_sandbox_broker::idempotency::IdempotencyStore;
use trogon_sandbox_contract::LeaseId;

use crate::{AdminError, Console};

/// Clears or retries the freeze marker of a lease.
pub async fn freeze(
    active: &dyn ActiveLeaseStore,
    idempotency: &dyn IdempotencyStore,
    lease_id: &LeaseId,
    resolution: FreezeResolution,
    force: bool,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    match resolve_unsettled_freeze(active, idempotency, lease_id, resolution, force)
        .await
        .map_err(AdminError::step("resolve freeze"))?
    {
        FreezeResolveOutcome::Applied => console.out("applied"),
        FreezeResolveOutcome::NoSuchLease => console.out("no such lease"),
        FreezeResolveOutcome::NoMarker => console.out("no freeze marker"),
        FreezeResolveOutcome::NotUnsettled { settle_attempts } => {
            Err(AdminError::FreezeStillSettling { settle_attempts })
        }
        FreezeResolveOutcome::Conflict => Err(AdminError::FreezeConflict),
    }
}
