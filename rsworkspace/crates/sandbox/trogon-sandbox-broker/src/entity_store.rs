//! Durable store for persistent sandbox entities.
//!
//! The broker only reaches this store through the shadow hooks in
//! `entity_shadow`, which exist only when `SANDBOX_ENTITY_MODE` is not `off`.
//!
//! Key layout (NATS KV):
//! - `sandbox-entities`: `e.{hex(tenant)}.{ulid}`
//! - `sandbox-entity-names`: `n.{hex(tenant)}.{hex(name)}` holding the entity id
//!
//! Hex is injective, and `.` never occurs inside a hex token, so distinct
//! (tenant, name) pairs can never share a key, and `e.{hex}.` is an exact
//! tenant prefix (`acme` never matches `acme_team`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

use async_nats::jetstream;
use async_nats::jetstream::kv::{self, Operation};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_contract::{EntityState, SandboxEntityId, SandboxEntityRecord, TenantId};
use trogon_sandbox_contract::{SnapshotTag, SnapshotTagName};
use trogon_sandbox_protocol::subjects::{ENTITY_KV_BUCKET, ENTITY_NAME_INDEX_KV_BUCKET};

use crate::generated_ulid::generated_ulid;
use trogon_sandbox_protocol::{EntityEvent, apply_entity_event};
mod in_memory_sandbox_entity_store;
mod nats_kv_sandbox_entity_store;
mod new_sandbox_entity;
mod versioned;

use crate::constants::{ENTITY_KV_HISTORY, MAX_CAS_RETRIES, MAX_ENTITY_NAME_BYTES};
pub use crate::entity_store_error::EntityStoreError;
pub use crate::sandbox_entity_mode::SandboxEntityMode;
pub use in_memory_sandbox_entity_store::InMemorySandboxEntityStore;
pub use nats_kv_sandbox_entity_store::NatsKvSandboxEntityStore;
pub use new_sandbox_entity::NewSandboxEntity;
pub use versioned::Versioned;
fn hex_token(value: &str) -> String {
    hex::encode(value.as_bytes())
}

fn decode_hex_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

pub fn entity_kv_key(tenant: &TenantId, id: &SandboxEntityId) -> String {
    format!("e.{}.{}", hex_token(tenant.as_str()), id)
}

pub fn entity_tenant_prefix(tenant: &TenantId) -> String {
    format!("e.{}.", hex_token(tenant.as_str()))
}

pub fn entity_name_kv_key(tenant: &TenantId, name: &str) -> String {
    format!("n.{}.{}", hex_token(tenant.as_str()), hex_token(name))
}

fn validate_name(name: &str) -> Result<(), EntityStoreError> {
    if name.is_empty() || name.len() > MAX_ENTITY_NAME_BYTES {
        return Err(EntityStoreError::InvalidArgument(format!(
            "name must be 1-{MAX_ENTITY_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

fn new_entity_id() -> Result<SandboxEntityId, EntityStoreError> {
    SandboxEntityId::new(generated_ulid()).map_err(|e| EntityStoreError::InvalidArgument(e.to_string()))
}

fn build_record(new: NewSandboxEntity, id: SandboxEntityId) -> Result<SandboxEntityRecord, EntityStoreError> {
    let persistence_tag = match new.persistence_tag {
        Some(tag) => tag,
        None => SnapshotTag {
            tenant: new.tenant.clone(),
            name: SnapshotTagName::new(format!("e-{id}"))
                .map_err(|e| EntityStoreError::InvalidArgument(e.to_string()))?,
        },
    };
    Ok(SandboxEntityRecord {
        entity_id: id,
        tenant: new.tenant,
        name: new.name,
        created_at: SystemTime::now(),
        created_by: new.created_by,
        spec: new.spec,
        metadata: new.metadata,
        drives: new.drives,
        persistence_tag,
        head: None,
        state: EntityState::Creating,
        generation: 0,
        current_lease: None,
    })
}

/// Mutator used by [`SandboxEntityStore::mutate`]. May run more than once.
pub type EntityMutator<'a> = &'a (dyn Fn(&mut SandboxEntityRecord) -> Result<(), EntityStoreError> + Send + Sync);

#[async_trait]
pub trait SandboxEntityStore: Send + Sync {
    /// Create an entity in `Creating`. A name, when given, is claimed
    /// atomically; a taken name yields [`EntityStoreError::NameTaken`] and
    /// leaves nothing behind.
    async fn create(&self, new: NewSandboxEntity) -> Result<Versioned, EntityStoreError>;

    /// Tenant-scoped read. Another tenant's id answers `None`.
    async fn get(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<Option<Versioned>, EntityStoreError>;

    async fn get_by_name(&self, tenant: &TenantId, name: &str) -> Result<Option<Versioned>, EntityStoreError>;

    /// All entities of one tenant, oldest first.
    async fn list(&self, tenant: &TenantId) -> Result<Vec<Versioned>, EntityStoreError>;

    /// Every entity of every tenant, for the repair sweep.
    async fn list_all(&self) -> Result<Vec<Versioned>, EntityStoreError>;

    /// Remove name claims whose entity is gone or no longer carries the name
    /// (a crash between delete steps). Returns how many were removed.
    async fn remove_dangling_name_claims(&self) -> Result<usize, EntityStoreError>;

    /// Write `record` only if the stored revision still equals `revision`.
    /// Identity fields (`entity_id`, `tenant`, `name`) must not change.
    async fn compare_and_swap(&self, record: &SandboxEntityRecord, revision: u64) -> Result<u64, EntityStoreError>;

    /// Remove the entity and release its name claim.
    async fn delete(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<(), EntityStoreError>;

    /// Read-modify-write with bounded CAS retries.
    async fn mutate(
        &self,
        tenant: &TenantId,
        id: &SandboxEntityId,
        mutator: EntityMutator<'_>,
    ) -> Result<Versioned, EntityStoreError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some(Versioned { mut record, revision }) = self.get(tenant, id).await? else {
                return Err(EntityStoreError::NotFound);
            };
            mutator(&mut record)?;
            match self.compare_and_swap(&record, revision).await {
                Ok(revision) => return Ok(Versioned { record, revision }),
                Err(EntityStoreError::RevisionConflict) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(EntityStoreError::CasRetriesExhausted)
    }

    /// Apply a lifecycle event through the transition table.
    async fn apply_event(
        &self,
        tenant: &TenantId,
        id: &SandboxEntityId,
        event: &EntityEvent,
    ) -> Result<Versioned, EntityStoreError> {
        self.mutate(tenant, id, &|record| {
            apply_entity_event(record, event)?;
            Ok(())
        })
        .await
    }
}

fn check_identity_unchanged(stored: &SandboxEntityRecord, next: &SandboxEntityRecord) -> Result<(), EntityStoreError> {
    if stored.entity_id != next.entity_id || stored.tenant != next.tenant || stored.name != next.name {
        return Err(EntityStoreError::InvalidArgument(
            "entity identity fields are immutable".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
