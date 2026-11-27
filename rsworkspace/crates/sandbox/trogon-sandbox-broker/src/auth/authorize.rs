use std::future::Future;

use async_nats::HeaderMap;
use tracing::warn;
use trogon_sandbox_contract::{CancelLeaseResponse, LeaseError, LeaseId, TenantId};

use super::{AuthContext, AuthDecision, ClientVerb, Unauthorized, VerbScope, authenticate_lease_request};

/// Gate for a [`VerbScope::Tenant`] verb: the bearer key must belong to
/// `tenant`. Always `Ok` when enforcement is off.
pub async fn authorize_tenant(
    ctx: &AuthContext,
    headers: Option<&HeaderMap>,
    verb: ClientVerb,
    tenant: &TenantId,
) -> Result<(), Unauthorized> {
    debug_assert_eq!(verb.scope(), VerbScope::Tenant, "{verb:?}");
    match authenticate_lease_request(ctx, headers, tenant).await {
        AuthDecision::Allowed { .. } => Ok(()),
        AuthDecision::Denied(reason) => {
            warn!(?verb, "client verb denied");
            Err(Unauthorized {
                reason: format!("{reason:?}"),
            })
        }
    }
}

/// Gate for a [`VerbScope::Lease`] verb: the bearer key must belong to the
/// lease's tenant. `owner` is only awaited when enforcement is on, so
/// disabled enforcement costs no lookup and leaves the handler's own
/// not-found behavior untouched.
pub async fn authorize_lease(
    ctx: &AuthContext,
    headers: Option<&HeaderMap>,
    verb: ClientVerb,
    lease_id: &LeaseId,
    owner: impl Future<Output = Option<TenantId>>,
) -> Result<(), LeaseError> {
    debug_assert_eq!(verb.scope(), VerbScope::Lease, "{verb:?}");
    if !ctx.enforce {
        return Ok(());
    }
    let Some(tenant) = owner.await else {
        return Err(LeaseError::LeaseNotFound {
            lease_id: lease_id.clone(),
        });
    };
    match authenticate_lease_request(ctx, headers, &tenant).await {
        AuthDecision::Allowed { .. } => Ok(()),
        AuthDecision::Denied(reason) => {
            warn!(?verb, lease_id = %lease_id, "client verb denied");
            Err(LeaseError::Unauthorized {
                reason: format!("{reason:?}"),
            })
        }
    }
}

/// Runs a client cancel behind the [`ClientVerb::Cancel`] gate. `cancel` is
/// only called once the caller is authorized, and its failure is reported as
/// a backend denial so the caller can tell a refused cancel from a done one.
pub async fn cancel_lease_for_client<F, Fut, E>(
    ctx: &AuthContext,
    headers: Option<&HeaderMap>,
    lease_id: &LeaseId,
    owner: impl Future<Output = Option<TenantId>>,
    cancel: F,
) -> CancelLeaseResponse
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    if let Err(denied) = authorize_lease(ctx, headers, ClientVerb::Cancel, lease_id, owner).await {
        warn!(lease_id = %lease_id, error = %denied, "cancel refused");
        return CancelLeaseResponse::Denied(denied);
    }
    match cancel().await {
        Ok(()) => CancelLeaseResponse::Cancelled,
        Err(e) => {
            warn!(error = %e, lease_id = %lease_id, "cancel failed");
            CancelLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() })
        }
    }
}

/// A file request is authorized against the lease its subject names, before
/// its payload is read, so the body must name that same lease.
pub fn require_subject_lease(subject_lease: &LeaseId, body_lease: &LeaseId) -> Result<(), LeaseError> {
    if subject_lease == body_lease {
        return Ok(());
    }
    warn!(subject_lease = %subject_lease, body_lease = %body_lease, "file request names a lease other than its subject");
    Err(LeaseError::Unauthorized {
        reason: "request lease does not match its subject".to_string(),
    })
}
