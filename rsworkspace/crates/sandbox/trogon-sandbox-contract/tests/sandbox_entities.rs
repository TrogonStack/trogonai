use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    ArtifactType, DeleteEntityRequest, DeleteEntityResponse, Digest, EntityLifecycleEvent, EntityRef, EntityState,
    EntityStateKind, GetEntityRequest, GetEntityResponse, LeaseError, LeaseId, ListEntitiesRequest,
    ListEntitiesResponse, SandboxEntityId, SandboxEntityIdError, SandboxEntityRecord, SandboxEntityRow, SnapshotHead,
    TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OTHER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn id() -> Result<SandboxEntityId, Box<dyn std::error::Error>> {
    Ok(SandboxEntityId::new(ID)?)
}

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn roundtrip<T>(value: &T, expected: &Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(&serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected.clone())?, value);
    Ok(())
}

#[test]
fn entity_id_accepts_a_ulid_in_either_case() -> TestResult {
    let parsed = SandboxEntityId::new(ID)?;
    assert_eq!(parsed.as_str(), ID);
    assert_eq!(parsed.to_string(), ID);
    assert_eq!(SandboxEntityId::LEN, 26);
    assert_eq!(ID.parse::<SandboxEntityId>()?, parsed);
    assert_eq!(SandboxEntityId::try_from(ID.to_owned())?, parsed);
    assert_eq!(SandboxEntityId::new(ID.to_ascii_lowercase())?, parsed);
    assert_eq!(String::from(parsed.clone()), ID);
    roundtrip(&parsed, &json!(ID))?;
    assert_eq!(
        serde_json::from_value::<SandboxEntityId>(json!(ID.to_ascii_lowercase()))?,
        parsed
    );
    Ok(())
}

#[test]
fn entity_id_rejects_a_wrong_length_or_alphabet() -> TestResult {
    let short = SandboxEntityId::LEN - 1;
    let length = SandboxEntityIdError::Length { expected: 26 };
    assert_eq!(error_of(SandboxEntityId::new(&ID[..short]))?, length);
    assert_eq!(error_of(SandboxEntityId::new(format!("{ID}0")))?, length);
    assert_eq!(
        error_of("".parse::<SandboxEntityId>())?.to_string(),
        "entity id must be 26 characters"
    );
    for bad in ["I", "L", "O", "U", "-", "\u{e9}"] {
        let keep = SandboxEntityId::LEN - bad.len();
        let value = format!("{}{bad}", &ID[..keep]);
        assert_eq!(value.len(), SandboxEntityId::LEN);
        let error = error_of(SandboxEntityId::try_from(value.clone()))?;
        assert_eq!(error, SandboxEntityIdError::InvalidCharacter, "{value}");
        assert_eq!(error.to_string(), "entity id must use Crockford base32 characters");
        assert_eq!(
            error_of(serde_json::from_value::<SandboxEntityId>(json!(value)))?.classify(),
            Data
        );
    }
    assert_eq!(
        error_of(serde_json::from_value::<SandboxEntityId>(json!("short")))?.to_string(),
        "entity id must be 26 characters"
    );
    Ok(())
}

fn head() -> Result<SnapshotHead, Box<dyn std::error::Error>> {
    Ok(SnapshotHead {
        digest: Digest::new(DIGEST)?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        captured_at: at(5),
        from_lease: LeaseId::new("lease-1"),
    })
}

fn head_json() -> Value {
    json!({
        "digest": DIGEST,
        "artifact_type": "sandbox_rootfs_v1",
        "captured_at": time(5),
        "from_lease": "lease-1",
    })
}

#[test]
fn snapshot_head_roundtrips() -> TestResult {
    roundtrip(&head()?, &head_json())
}

#[test]
fn entity_state_roundtrips_and_reports_its_kind_and_lease() -> TestResult {
    let lease = LeaseId::new("lease-1");
    let cases = [
        (
            EntityState::Creating,
            json!("creating"),
            EntityStateKind::Creating,
            None,
        ),
        (
            EntityState::Starting { lease: lease.clone() },
            json!({ "starting": { "lease": "lease-1" } }),
            EntityStateKind::Starting,
            Some(&lease),
        ),
        (
            EntityState::Running { lease: lease.clone() },
            json!({ "running": { "lease": "lease-1" } }),
            EntityStateKind::Running,
            Some(&lease),
        ),
        (
            EntityState::Paused { lease: lease.clone() },
            json!({ "paused": { "lease": "lease-1" } }),
            EntityStateKind::Paused,
            Some(&lease),
        ),
        (
            EntityState::Stopping { lease: lease.clone() },
            json!({ "stopping": { "lease": "lease-1" } }),
            EntityStateKind::Stopping,
            Some(&lease),
        ),
        (EntityState::Stopped, json!("stopped"), EntityStateKind::Stopped, None),
        (
            EntityState::Deleting,
            json!("deleting"),
            EntityStateKind::Deleting,
            None,
        ),
        (
            EntityState::Failed {
                reason: "boom".to_owned(),
            },
            json!({ "failed": { "reason": "boom" } }),
            EntityStateKind::Failed,
            None,
        ),
    ];
    for (state, wire, kind, bound) in cases {
        roundtrip(&state, &wire)?;
        assert_eq!(state.kind(), kind);
        assert_eq!(state.lease(), bound);
    }
    Ok(())
}

fn lifecycle_cases() -> Result<Vec<(EntityLifecycleEvent, Value)>, Box<dyn std::error::Error>> {
    let tenant = TenantId::new("acme")?;
    Ok(vec![
        (
            EntityLifecycleEvent::Created {
                entity_id: id()?,
                tenant: tenant.clone(),
                name: Some("dev".to_owned()),
                at: at(1),
            },
            json!({ "event": "created", "entity_id": ID, "tenant": "acme", "name": "dev", "at": time(1) }),
        ),
        (
            EntityLifecycleEvent::Created {
                entity_id: id()?,
                tenant: tenant.clone(),
                name: None,
                at: at(1),
            },
            json!({ "event": "created", "entity_id": ID, "tenant": "acme", "at": time(1) }),
        ),
        (
            EntityLifecycleEvent::RunStarted {
                entity_id: id()?,
                tenant: tenant.clone(),
                lease: LeaseId::new("lease-1"),
                generation: 2,
                from_digest: Some(Digest::new(DIGEST)?),
                at: at(2),
            },
            json!({
                "event": "run_started", "entity_id": ID, "tenant": "acme", "lease": "lease-1",
                "generation": 2, "from_digest": DIGEST, "at": time(2),
            }),
        ),
        (
            EntityLifecycleEvent::RunStarted {
                entity_id: id()?,
                tenant: tenant.clone(),
                lease: LeaseId::new("lease-1"),
                generation: 1,
                from_digest: None,
                at: at(2),
            },
            json!({
                "event": "run_started", "entity_id": ID, "tenant": "acme", "lease": "lease-1",
                "generation": 1, "at": time(2),
            }),
        ),
        (
            EntityLifecycleEvent::RunEnded {
                entity_id: id()?,
                tenant: tenant.clone(),
                lease: LeaseId::new("lease-1"),
                generation: 2,
                reason: "completed".to_owned(),
                captured_digest: Some(Digest::new(DIGEST)?),
                at: at(3),
            },
            json!({
                "event": "run_ended", "entity_id": ID, "tenant": "acme", "lease": "lease-1",
                "generation": 2, "reason": "completed", "captured_digest": DIGEST, "at": time(3),
            }),
        ),
        (
            EntityLifecycleEvent::RunEnded {
                entity_id: id()?,
                tenant: tenant.clone(),
                lease: LeaseId::new("lease-1"),
                generation: 2,
                reason: "lost".to_owned(),
                captured_digest: None,
                at: at(3),
            },
            json!({
                "event": "run_ended", "entity_id": ID, "tenant": "acme", "lease": "lease-1",
                "generation": 2, "reason": "lost", "at": time(3),
            }),
        ),
        (
            EntityLifecycleEvent::Deleted {
                entity_id: id()?,
                tenant,
                at: at(4),
            },
            json!({ "event": "deleted", "entity_id": ID, "tenant": "acme", "at": time(4) }),
        ),
    ])
}

#[test]
fn lifecycle_events_roundtrip_and_expose_entity_and_tenant() -> TestResult {
    for (event, wire) in lifecycle_cases()? {
        roundtrip(&event, &wire)?;
        assert_eq!(event.entity_id(), &id()?);
        assert_eq!(event.tenant(), &TenantId::new("acme")?);
    }
    Ok(())
}

#[test]
fn lifecycle_event_rejects_an_invalid_entity_id() -> TestResult {
    let wire = json!({ "event": "deleted", "entity_id": "nope", "tenant": "acme", "at": time(4) });
    assert_eq!(
        error_of(serde_json::from_value::<EntityLifecycleEvent>(wire))?.classify(),
        Data
    );
    Ok(())
}

fn record_json() -> Value {
    json!({
        "entity_id": ID,
        "tenant": "acme",
        "name": "dev",
        "created_at": time(1),
        "created_by": "svc-1",
        "spec": {
            "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
            "workspace": { "empty": {} },
            "cpu_seconds": 10,
            "memory_mib": 512,
        },
        "metadata": { "team": "core" },
        "drives": [{ "drive_id": { "tenant": "acme", "name": "d0" }, "mount_path": "/data", "mode": "read_only" }],
        "persistence_tag": { "tenant": "acme", "name": "persist" },
        "head": head_json(),
        "state": { "running": { "lease": "lease-1" } },
        "generation": 3,
        "current_lease": "lease-1",
    })
}

fn row_json() -> Value {
    json!({
        "entity_id": ID,
        "tenant": "acme",
        "name": "dev",
        "created_at": time(1),
        "metadata": { "team": "core" },
        "drives": [{ "drive_id": { "tenant": "acme", "name": "d0" }, "mount_path": "/data", "mode": "read_only" }],
        "head": head_json(),
        "state": { "running": { "lease": "lease-1" } },
        "generation": 3,
        "current_lease": "lease-1",
    })
}

#[test]
fn record_roundtrips_and_converts_to_a_row_without_the_spec() -> TestResult {
    let record = serde_json::from_value::<SandboxEntityRecord>(record_json())?;
    let wire = serde_json::to_value(&record)?;
    assert_eq!(wire["entity_id"], json!(ID));
    assert_eq!(wire["persistence_tag"], json!({ "tenant": "acme", "name": "persist" }));
    assert_eq!(serde_json::from_value::<SandboxEntityRecord>(wire)?, record);

    let row = SandboxEntityRow::from(&record);
    roundtrip(&row, &row_json())?;
    assert_eq!(row.generation, 3);
    assert!(serde_json::to_value(&row)?.get("spec").is_none());
    Ok(())
}

#[test]
fn record_and_row_omit_absent_optional_fields_and_default_the_rest() -> TestResult {
    let mut minimal = record_json();
    for key in [
        "name",
        "created_by",
        "metadata",
        "drives",
        "head",
        "generation",
        "current_lease",
    ] {
        minimal.as_object_mut().ok_or("object")?.remove(key);
    }
    minimal["state"] = json!("stopped");
    let record = serde_json::from_value::<SandboxEntityRecord>(minimal)?;
    assert_eq!(record.generation, 0);
    let wire = serde_json::to_value(&record)?;
    for key in ["name", "created_by", "drives", "head", "current_lease"] {
        assert!(wire.get(key).is_none(), "{key}");
    }
    assert_eq!(wire["generation"], json!(0));

    let row = SandboxEntityRow::from(&record);
    let row_wire = serde_json::to_value(&row)?;
    assert_eq!(row_wire["state"], json!("stopped"));
    assert!(row_wire.get("name").is_none());
    assert_eq!(serde_json::from_value::<SandboxEntityRow>(row_wire)?, row);
    Ok(())
}

#[test]
fn entity_ref_roundtrips_by_id_or_name() -> TestResult {
    roundtrip(&EntityRef::Id(id()?), &json!({ "id": ID }))?;
    roundtrip(&EntityRef::Name("dev".to_owned()), &json!({ "name": "dev" }))?;
    assert_eq!(
        error_of(serde_json::from_value::<EntityRef>(json!({ "id": "bad" })))?.classify(),
        Data
    );
    Ok(())
}

#[test]
fn get_and_delete_requests_roundtrip() -> TestResult {
    let tenant = TenantId::new("acme")?;
    roundtrip(
        &GetEntityRequest {
            tenant: tenant.clone(),
            entity: EntityRef::Id(id()?),
        },
        &json!({ "tenant": "acme", "entity": { "id": ID } }),
    )?;
    roundtrip(
        &DeleteEntityRequest {
            tenant,
            entity: EntityRef::Name("dev".to_owned()),
        },
        &json!({ "tenant": "acme", "entity": { "name": "dev" } }),
    )
}

#[test]
fn get_response_roundtrips_each_outcome() -> TestResult {
    let row = serde_json::from_value(row_json())?;
    roundtrip(
        &GetEntityResponse::Found { row },
        &json!({ "outcome": "found", "row": row_json() }),
    )?;
    roundtrip(&GetEntityResponse::NotFound, &json!({ "outcome": "not_found" }))?;
    roundtrip(
        &GetEntityResponse::Denied(LeaseError::NoProviderAvailable),
        &json!({ "outcome": "denied", "kind": "no_provider_available" }),
    )
}

#[test]
fn delete_response_roundtrips_each_outcome() -> TestResult {
    roundtrip(&DeleteEntityResponse::Deleted, &json!({ "outcome": "deleted" }))?;
    roundtrip(&DeleteEntityResponse::NotFound, &json!({ "outcome": "not_found" }))?;
    roundtrip(
        &DeleteEntityResponse::Denied(LeaseError::NoProviderAvailable),
        &json!({ "outcome": "denied", "kind": "no_provider_available" }),
    )
}

#[test]
fn list_request_roundtrips_and_bounds_its_page_size() -> TestResult {
    let tenant = TenantId::new("acme")?;
    let bare = ListEntitiesRequest {
        tenant: tenant.clone(),
        limit: None,
        cursor: None,
    };
    roundtrip(&bare, &json!({ "tenant": "acme" }))?;
    assert_eq!(bare.page_size(), ListEntitiesRequest::DEFAULT_LIMIT);
    assert_eq!(ListEntitiesRequest::DEFAULT_LIMIT, 100);
    assert_eq!(ListEntitiesRequest::MAX_LIMIT, 500);

    let paged = ListEntitiesRequest {
        tenant,
        limit: Some(7),
        cursor: Some(SandboxEntityId::new(OTHER_ID)?),
    };
    roundtrip(&paged, &json!({ "tenant": "acme", "limit": 7, "cursor": OTHER_ID }))?;
    assert_eq!(paged.page_size(), 7);
    for (limit, size) in [(0, 1), (1, 1), (500, 500), (501, 500), (u32::MAX, 500)] {
        let request = ListEntitiesRequest {
            limit: Some(limit),
            ..bare.clone()
        };
        assert_eq!(request.page_size(), size);
    }
    Ok(())
}

#[test]
fn list_response_roundtrips_each_outcome() -> TestResult {
    let row = serde_json::from_value(row_json())?;
    roundtrip(
        &ListEntitiesResponse::Listed {
            rows: vec![row],
            next_cursor: Some(SandboxEntityId::new(OTHER_ID)?),
        },
        &json!({ "outcome": "listed", "rows": [row_json()], "next_cursor": OTHER_ID }),
    )?;
    roundtrip(
        &ListEntitiesResponse::Listed {
            rows: vec![],
            next_cursor: None,
        },
        &json!({ "outcome": "listed", "rows": [] }),
    )?;
    roundtrip(
        &ListEntitiesResponse::Denied(LeaseError::NoProviderAvailable),
        &json!({ "outcome": "denied", "kind": "no_provider_available" }),
    )
}
