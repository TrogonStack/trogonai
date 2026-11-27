use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use trogon_sandbox_contract::{
    Digest, DriveId, DriveName, DriveStorage, EntityRef, LeaseId, ListEntitiesRequest, ProviderVolumeRef,
    SandboxEntityId, SnapshotTagName, TenantId,
};

use crate::PayloadError;
use crate::from_payload::from_payload;

#[derive(Deserialize)]
struct DrivePayload {
    drive_name: DriveName,
}

#[derive(Deserialize)]
struct CreateDrivePayload {
    drive_name: DriveName,
    #[serde(default)]
    size_gib: Option<u64>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    provider_volume: Option<ProviderVolumeRef>,
}

#[derive(Deserialize)]
struct EntityPayload {
    #[serde(default)]
    entity_id: Option<SandboxEntityId>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct ListEntitiesPayload {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<SandboxEntityId>,
}

#[derive(Deserialize)]
struct NamePayload<T> {
    name: T,
}

#[derive(Deserialize)]
struct DigestPayload {
    digest: Digest,
}

#[derive(Deserialize)]
struct SnapshotTagPayload {
    name: SnapshotTagName,
    digest: Digest,
}

#[derive(Deserialize)]
struct DomainPayload {
    lease_id: LeaseId,
    port: u16,
    #[serde(default)]
    wait_ms: Option<u64>,
}

/// A drive id in the session's tenant, never one the payload names.
pub(crate) fn drive_id(payload: &Value, tenant: &TenantId) -> Result<DriveId, PayloadError> {
    let drive: DrivePayload = from_payload(payload)?;
    Ok(DriveId {
        tenant: tenant.clone(),
        name: drive.drive_name,
    })
}

/// The drive to create. Its storage is either an S3 `bucket` or a `provider_volume`; exactly one must be present.
pub(crate) fn create_drive(
    payload: &Value,
    tenant: &TenantId,
) -> Result<(DriveId, Option<u64>, DriveStorage), PayloadError> {
    let drive: CreateDrivePayload = from_payload(payload)?;
    let storage = match (drive.bucket, drive.provider_volume) {
        (Some(_), Some(_)) => return Err(PayloadError::AmbiguousDriveStorage),
        (Some(bucket), None) => DriveStorage::S3 { bucket },
        (None, Some(provider_volume)) => DriveStorage::ProviderVolume { provider_volume },
        (None, None) => return Err(PayloadError::MissingDriveStorage),
    };
    let id = DriveId {
        tenant: tenant.clone(),
        name: drive.drive_name,
    };
    Ok((id, drive.size_gib, storage))
}

/// Exactly one of `entity_id` or `name`.
pub(crate) fn entity_ref(payload: &Value) -> Result<EntityRef, PayloadError> {
    let entity: EntityPayload = from_payload(payload)?;
    match (entity.entity_id, entity.name) {
        (Some(id), None) => Ok(EntityRef::Id(id)),
        (None, Some(name)) => Ok(EntityRef::Name(name)),
        (None, None) => Err(PayloadError::MissingEntity),
        (Some(_), Some(_)) => Err(PayloadError::AmbiguousEntity),
    }
}

pub(crate) fn list_entities(payload: &Value, tenant: &TenantId) -> Result<ListEntitiesRequest, PayloadError> {
    let list: ListEntitiesPayload = from_payload(payload)?;
    Ok(ListEntitiesRequest {
        tenant: tenant.clone(),
        limit: list.limit,
        cursor: list.cursor,
    })
}

/// The `name` a payload carries, for named sandboxes and snapshot tags.
pub(crate) fn name<T: DeserializeOwned>(payload: &Value) -> Result<T, PayloadError> {
    Ok(from_payload::<NamePayload<T>>(payload)?.name)
}

pub(crate) fn digest(payload: &Value) -> Result<Digest, PayloadError> {
    Ok(from_payload::<DigestPayload>(payload)?.digest)
}

pub(crate) fn snapshot_tag(payload: &Value) -> Result<(SnapshotTagName, Digest), PayloadError> {
    let tag: SnapshotTagPayload = from_payload(payload)?;
    Ok((tag.name, tag.digest))
}

/// The lease, port and optional wait of `sandbox_get_domain`.
pub(crate) fn domain(payload: &Value) -> Result<(LeaseId, u16, Option<u64>), PayloadError> {
    let domain: DomainPayload = from_payload(payload)?;
    Ok((domain.lease_id, domain.port, domain.wait_ms))
}
