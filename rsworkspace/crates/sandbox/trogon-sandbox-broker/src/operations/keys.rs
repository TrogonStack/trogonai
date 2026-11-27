use sha2::Digest as _;
use trogon_sandbox_contract::{LeaseId, OperationId, OperationRequest, TenantId};

use super::operations_error::OperationsError;
use crate::image_labels::operation_kind;

pub fn op_key(tenant: &TenantId, id: &OperationId) -> String {
    format!("op.{}.{id}", hex::encode(tenant.as_str().as_bytes()))
}

pub fn active_key(tenant: &TenantId, id: &OperationId) -> String {
    format!("active.{}.{id}", hex::encode(tenant.as_str().as_bytes()))
}

pub fn target_key(lease: &LeaseId) -> String {
    format!("target.{}", hex::encode(lease.as_str().as_bytes()))
}

pub(super) fn cancellation_key(tenant: &TenantId, id: &OperationId) -> String {
    format!("cancel.{}.{id}", hex::encode(tenant.as_str().as_bytes()))
}

pub(super) fn cleanup_key(tenant: &TenantId, id: &OperationId) -> String {
    format!("cleanup.{}.{id}", hex::encode(tenant.as_str().as_bytes()))
}

pub(super) fn cleanup_active_key(tenant: &TenantId, id: &OperationId) -> String {
    format!("cleanup_active.{}.{id}", hex::encode(tenant.as_str().as_bytes()))
}

pub(super) fn parse_active_key(key: &str) -> Option<(TenantId, OperationId)> {
    let rest = key.strip_prefix("active.")?;
    let (tenant, id) = rest.split_once('.')?;
    let tenant = String::from_utf8(hex::decode(tenant).ok()?).ok()?;
    Some((TenantId::new(tenant).ok()?, OperationId::new(id).ok()?))
}

/// Hash of what the request asks for, so an operation id reused for a
/// different request is caught. The operation id itself is not part of it.
///
/// A create is identified by what it asks for alone: the lease and attempt ids
/// are chosen by the broker, so a client retry cannot know them.
pub fn fingerprint(tenant: &TenantId, lease: &LeaseId, request: &OperationRequest) -> Result<String, OperationsError> {
    let canonical = match request {
        OperationRequest::Create { request: ask, .. } => serde_json::to_vec(&(tenant, operation_kind(request), ask)),
        _ => serde_json::to_vec(&(tenant, operation_kind(request), lease, request)),
    }?;
    Ok(hex::encode(sha2::Sha256::digest(canonical)))
}
