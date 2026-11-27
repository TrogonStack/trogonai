//! Reads and writes of NATS KV records scoped to a tenant alone (limits,
//! image and host_mount allow-lists), keyed by
//! [`trogon_sandbox_protocol::tenant_kv_key`].
//!
//! Records written before that key layout sit under
//! [`trogon_sandbox_protocol::legacy_tenant_kv_key`], which tenants such as
//! `a.b` and `a_b` share. Those records carry no owner, so a read falls back
//! to the legacy key only while the tenant has no record under its current
//! key, which is what every tenant read before this layout. Writing a record
//! for a tenant ends that fallback for it, and clearing a tenant removes the
//! legacy record too so a cleared policy cannot come back from it.

mod tenant_record_error;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{legacy_tenant_kv_key, tenant_kv_key};

use crate::constants::TENANT_RECORD_MAX_AGE;

pub use tenant_record_error::TenantRecordError;

/// Opens `bucket`, creating it with the tenant record retention when it does not exist.
pub async fn open_bucket(jetstream: &jetstream::Context, bucket: &str) -> Result<kv::Store, TenantRecordError> {
    jetstream
        .create_or_update_key_value(kv::Config {
            bucket: bucket.to_string(),
            history: 1,
            max_age: TENANT_RECORD_MAX_AGE,
            ..Default::default()
        })
        .await
        .map_err(TenantRecordError::backend)
}

/// Current record of `tenant`, parsed, falling back to its legacy key.
pub async fn get_parsed<T: DeserializeOwned>(
    store: &kv::Store,
    tenant: &TenantId,
) -> Result<Option<T>, TenantRecordError> {
    match get(store, tenant).await? {
        None => Ok(None),
        Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
    }
}

/// Current record of `tenant`, falling back to its legacy key.
pub async fn get(store: &kv::Store, tenant: &TenantId) -> Result<Option<Bytes>, TenantRecordError> {
    let key = tenant_kv_key(tenant);
    if let Some(value) = store.get(&key).await.map_err(TenantRecordError::backend)? {
        return Ok(Some(value));
    }
    let legacy = legacy_tenant_kv_key(tenant);
    if legacy == key {
        return Ok(None);
    }
    store.get(&legacy).await.map_err(TenantRecordError::backend)
}

/// Writes `value` under the current key of `tenant`.
pub async fn put(store: &kv::Store, tenant: &TenantId, value: Bytes) -> Result<(), TenantRecordError> {
    store
        .put(tenant_kv_key(tenant), value)
        .await
        .map_err(TenantRecordError::backend)?;
    Ok(())
}

/// Deletes the record of `tenant` under its current and legacy keys.
/// Returns whether either key held a record.
pub async fn clear(store: &kv::Store, tenant: &TenantId) -> Result<bool, TenantRecordError> {
    let mut keys = vec![tenant_kv_key(tenant)];
    let legacy = legacy_tenant_kv_key(tenant);
    if legacy != keys[0] {
        keys.push(legacy);
    }
    let mut found = false;
    for key in &keys {
        if store.get(key).await.map_err(TenantRecordError::backend)?.is_some() {
            store.delete(key).await.map_err(TenantRecordError::backend)?;
            found = true;
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
