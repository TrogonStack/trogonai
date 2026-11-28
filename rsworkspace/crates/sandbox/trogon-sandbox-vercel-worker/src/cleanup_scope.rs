use trogon_sandbox_contract::ProviderCleanupRequest;
use trogon_sandbox_protocol::VerbScope;

/// Why a targeted cleanup was refused before it reached the dispatch ledger.
pub(crate) const INVALID_CLEANUP_SCOPE: &str = "targeted cleanup requires exact verified assignment authority";

/// Whether `scope` was signed for exactly the lease, worker, attempt and epoch `request` names.
pub(crate) fn names_assignment(scope: &VerbScope, request: &ProviderCleanupRequest) -> bool {
    let identity = &request.identity;
    scope.lease_id == identity.lease_id
        && scope.worker_id.as_ref() == Some(&identity.target.worker_id)
        && scope.attempt_id.as_ref() == Some(&identity.target.attempt_id)
        && scope.epoch == identity.target.epoch
}

#[cfg(test)]
mod tests;
