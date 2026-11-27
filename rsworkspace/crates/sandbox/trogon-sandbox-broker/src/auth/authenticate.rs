use async_nats::HeaderMap;
use tracing::{info, warn};
use trogon_sandbox_auth::{AuthzError, BearerToken, SandboxAuthzRequest};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use super::{AuthContext, AuthDecision, AuthDenialReason};
use crate::constants::{AUTHORIZATION_HEADER, BEARER_PREFIX, UNAUTHENTICATED_PRINCIPAL};

#[tracing::instrument(skip_all, fields(tenant = %requested_tenant))]
pub async fn authenticate_lease_request(
    ctx: &AuthContext,
    headers: Option<&HeaderMap>,
    requested_tenant: &TenantId,
) -> AuthDecision {
    if !ctx.enforce {
        return AuthDecision::Allowed {
            tenant: requested_tenant.clone(),
            principal: ServiceAccountId::new(UNAUTHENTICATED_PRINCIPAL),
        };
    }

    let bearer = match extract_bearer_from_nats_headers(headers) {
        Some(token) => token,
        None => {
            warn!(tenant = %requested_tenant, "auth denied: missing bearer header");
            return AuthDecision::Denied(AuthDenialReason::MissingHeader);
        }
    };

    let key_hash = bearer.hash();
    let record = match ctx.api_keys.lookup(&key_hash).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!(tenant = %requested_tenant, "auth denied: unknown API key");
            return AuthDecision::Denied(AuthDenialReason::UnknownKey);
        }
        Err(e) => {
            warn!(tenant = %requested_tenant, error = %e, "auth denied: key store error");
            return AuthDecision::Denied(AuthDenialReason::Backend(e.to_string()));
        }
    };

    if record.tenant() != requested_tenant {
        warn!(tenant = %requested_tenant, "auth denied: tenant mismatch");
        return AuthDecision::Denied(AuthDenialReason::TenantMismatch);
    }

    let authz_req = SandboxAuthzRequest {
        tenant: record.tenant().clone(),
        principal: record.service_account().clone(),
    };

    match ctx.authorizer.authorize_sandbox_lease(&authz_req).await {
        Ok(()) => {
            info!(tenant = %requested_tenant, "auth allowed");
            AuthDecision::Allowed {
                tenant: record.tenant().clone(),
                principal: record.service_account().clone(),
            }
        }
        Err(AuthzError::Denied) => {
            warn!(tenant = %requested_tenant, "auth denied: permission denied by authorizer");
            AuthDecision::Denied(AuthDenialReason::PermissionDenied)
        }
        Err(AuthzError::Backend(e)) => {
            warn!(tenant = %requested_tenant, error = %e, "auth denied: authorizer backend error");
            AuthDecision::Denied(AuthDenialReason::Backend(e.to_string()))
        }
    }
}

pub(super) fn extract_bearer_from_nats_headers(headers: Option<&HeaderMap>) -> Option<BearerToken> {
    let headers = headers?;
    let value = headers.get(AUTHORIZATION_HEADER)?;
    let token = value.as_str().strip_prefix(BEARER_PREFIX)?;
    BearerToken::new(token).ok()
}
