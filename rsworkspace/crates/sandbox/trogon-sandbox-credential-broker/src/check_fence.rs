use trogon_sandbox_contract::{PolicyFence, PullSecretId, TenantId};

use crate::{CommittedPolicyStore, FenceMode, FenceRefusal};

/// Decides whether `reference` may be resolved for `tenant` under `fence`.
///
/// The broker resolves a credential only when the request names a lease and a policy revision, and the committed
/// policy for that tenant, lease and revision names the reference. A stale revision, a revision ahead of the
/// commit, an unknown lease, another tenant's lease and a reference the policy does not name are all refused the
/// same way, so a refusal reveals nothing about which check failed. A request without a fence is decided by
/// `mode`.
pub async fn check_fence(
    store: Option<&dyn CommittedPolicyStore>,
    mode: FenceMode,
    tenant: &TenantId,
    fence: Option<&PolicyFence>,
    reference: &PullSecretId,
) -> Result<(), FenceRefusal> {
    let Some(fence) = fence else {
        return match mode {
            FenceMode::Optional => Ok(()),
            FenceMode::Required => Err(FenceRefusal::Missing),
        };
    };
    let Some(store) = store else {
        return Err(FenceRefusal::NotCommitted);
    };
    let committed = store
        .committed(tenant, &fence.lease_id)
        .await
        .map_err(|_| FenceRefusal::Unavailable)?
        .ok_or(FenceRefusal::NotCommitted)?;
    if committed.revision != fence.policy_revision || !committed.references.contains(reference) {
        return Err(FenceRefusal::NotCommitted);
    }
    Ok(())
}
