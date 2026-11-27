use sha2::Digest as _;
use trogon_sandbox_contract::{LeaseRecord, LeaseRequestKey, TenantId};

use super::IdempotencyStoreError;

pub(super) fn storage_key(tenant: &TenantId, key: &str) -> String {
    let sanitized_tenant = tenant.as_str().replace(':', "_");
    format!("{sanitized_tenant}_{key}")
}

pub(super) fn attempt_key(tenant: &TenantId, key: &LeaseRequestKey) -> String {
    let identity = serde_json::Value::Array(vec![tenant.as_str().into(), key.as_str().into()]);
    format!("attempt.{}", hex::encode(sha2::Sha256::digest(identity.to_string())))
}

pub(super) fn validate_attempt(
    tenant: &TenantId,
    key: &LeaseRequestKey,
    record: &LeaseRecord,
) -> Result<(), IdempotencyStoreError> {
    if record.tenant == *tenant
        && record.worker_id.is_some()
        && record
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.idempotency_key == *key)
    {
        return Ok(());
    }
    Err(IdempotencyStoreError::JournalIdentityMismatch)
}

pub(super) fn validate_attempt_replacement(
    expected: &LeaseRecord,
    record: &LeaseRecord,
) -> Result<(), IdempotencyStoreError> {
    let before = expected
        .dispatch_attempt
        .as_ref()
        .ok_or(IdempotencyStoreError::ExpectedAttemptMissing)?;
    let after = record
        .dispatch_attempt
        .as_ref()
        .ok_or(IdempotencyStoreError::ReplacementAttemptMissing)?;
    if !(record.lease_id == expected.lease_id
        && record.worker_id == expected.worker_id
        && before.id == after.id
        && before.idempotency_key == after.idempotency_key
        && before.accounting == after.accounting)
    {
        return Err(IdempotencyStoreError::ReplacementChangesIdentity);
    }
    if before.cleanup_reason.is_some() && before.cleanup_reason != after.cleanup_reason {
        return Err(IdempotencyStoreError::ReplacementChangesCleanupReason);
    }
    if before.cleanup_started_at.is_some() && before.cleanup_started_at != after.cleanup_started_at {
        return Err(IdempotencyStoreError::ReplacementChangesCleanupTime);
    }
    Ok(())
}
