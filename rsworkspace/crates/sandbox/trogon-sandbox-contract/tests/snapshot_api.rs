use std::fmt::Debug;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, DeleteSnapshotTagRequest, DeleteSnapshotTagResponse, Digest,
    GetLeaseByNameRequest, GetLeaseByNameResponse, LeaseError, LeaseId, LeaseListRow, LeaseSnapshotReceipt,
    LeaseStatus, NativeSnapshotAcceptance, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, NativeSnapshotId,
    NativeSnapshotReceipt, NativeSnapshotRef, OciArtifactRef, ProviderKind, SandboxTags, SetSnapshotTagRequest,
    SetSnapshotTagResponse, SnapshotDeleteRequest, SnapshotDeleteResponse, SnapshotExportError, SnapshotExportRequest,
    SnapshotGetRequest, SnapshotGetResponse, SnapshotLeaseRequest, SnapshotLeaseResponse, SnapshotListRow,
    SnapshotPushTarget, SnapshotTagAuditEntry, SnapshotTagHistoryRequest, SnapshotTagHistoryResponse, SnapshotTagName,
    SnapshotTreeNode, SnapshotTreeRequest, SnapshotTreeResponse, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST_A: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DIGEST_B: &str = "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn digest(value: &str) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(value.parse()?)
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("acme")?)
}

fn tag_name() -> Result<SnapshotTagName, Box<dyn std::error::Error>> {
    Ok(SnapshotTagName::new("prod")?)
}

fn lease_denied() -> (LeaseError, Value) {
    (
        LeaseError::LeaseNotFound {
            lease_id: LeaseId::new("lease-1"),
        },
        json!({ "kind": "lease_not_found", "lease_id": "lease-1" }),
    )
}

fn check<T>(value: T, expected: Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    assert_eq!(serde_json::to_value(&value)?, expected);
    assert_eq!(serde_json::from_value::<T>(expected)?, value);
    Ok(())
}

fn rejects<T: DeserializeOwned + Debug>(input: Value) -> TestResult {
    let error = serde_json::from_value::<T>(input)
        .err()
        .ok_or("the input should be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

fn oci_ref() -> Result<(OciArtifactRef, Value), Box<dyn std::error::Error>> {
    Ok((
        OciArtifactRef {
            registry: "registry.example".to_owned(),
            repository: "snapshots/acme".to_owned(),
            digest: digest(DIGEST_A)?,
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        json!({
            "registry": "registry.example",
            "repository": "snapshots/acme",
            "digest": DIGEST_A,
            "artifact_type": "sandbox_rootfs_v1",
        }),
    ))
}

fn native_ref() -> Result<(NativeSnapshotRef, Value), Box<dyn std::error::Error>> {
    Ok((
        NativeSnapshotRef {
            provider: ProviderKind::Mock,
            id: NativeSnapshotId::new("snap-1")?,
            expires_at: None,
        },
        json!({ "provider": "mock", "id": "snap-1" }),
    ))
}

fn push_target() -> (SnapshotPushTarget, Value) {
    (
        SnapshotPushTarget {
            registry: "registry.example".to_owned(),
            repository: "snapshots/acme".to_owned(),
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        json!({
            "registry": "registry.example",
            "repository": "snapshots/acme",
            "artifact_type": "sandbox_rootfs_v1",
        }),
    )
}

#[test]
fn snapshot_list_rows_omit_an_absent_parent() -> TestResult {
    let root = SnapshotListRow {
        digest: digest(DIGEST_A)?,
        tenant: tenant()?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        created_at: at(5),
        parent: None,
    };
    check(
        root.clone(),
        json!({
            "digest": DIGEST_A,
            "tenant": "acme",
            "artifact_type": "sandbox_rootfs_v1",
            "created_at": time(5),
        }),
    )?;
    let child = SnapshotListRow {
        parent: Some(digest(DIGEST_B)?),
        ..root
    };
    check(
        child,
        json!({
            "digest": DIGEST_A,
            "tenant": "acme",
            "artifact_type": "sandbox_rootfs_v1",
            "created_at": time(5),
            "parent": DIGEST_B,
        }),
    )
}

#[test]
fn lease_list_rows_omit_absent_names_and_empty_tags() -> TestResult {
    let bare = LeaseListRow {
        lease_id: LeaseId::new("lease-1"),
        tenant: tenant()?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: at(1),
        deadline: at(2),
        name: None,
        tags: SandboxTags::default(),
    };
    check(
        bare.clone(),
        json!({
            "lease_id": "lease-1",
            "tenant": "acme",
            "provider": "mock",
            "status": "running",
            "created_at": time(1),
            "deadline": time(2),
        }),
    )?;
    let named = LeaseListRow {
        name: Some("build".parse()?),
        tags: serde_json::from_value(json!({ "env": "prod" }))?,
        ..bare
    };
    check(
        named,
        json!({
            "lease_id": "lease-1",
            "tenant": "acme",
            "provider": "mock",
            "status": "running",
            "created_at": time(1),
            "deadline": time(2),
            "name": "build",
            "tags": { "env": "prod" },
        }),
    )
}

#[test]
fn requests_that_name_a_digest_round_trip() -> TestResult {
    check(
        SnapshotGetRequest {
            tenant: tenant()?,
            digest: digest(DIGEST_A)?,
        },
        json!({ "tenant": "acme", "digest": DIGEST_A }),
    )?;
    check(
        SnapshotTreeRequest {
            tenant: tenant()?,
            digest: digest(DIGEST_A)?,
        },
        json!({ "tenant": "acme", "digest": DIGEST_A }),
    )?;
    check(
        SnapshotDeleteRequest {
            tenant: tenant()?,
            digest: digest(DIGEST_A)?,
        },
        json!({ "tenant": "acme", "digest": DIGEST_A }),
    )?;
    check(
        SetSnapshotTagRequest {
            tenant: tenant()?,
            name: tag_name()?,
            digest: digest(DIGEST_A)?,
        },
        json!({ "tenant": "acme", "name": "prod", "digest": DIGEST_A }),
    )
}

#[test]
fn requests_that_name_a_tag_round_trip_and_reject_a_bad_name() -> TestResult {
    check(
        DeleteSnapshotTagRequest {
            tenant: tenant()?,
            name: tag_name()?,
        },
        json!({ "tenant": "acme", "name": "prod" }),
    )?;
    check(
        SnapshotTagHistoryRequest {
            tenant: tenant()?,
            name: tag_name()?,
        },
        json!({ "tenant": "acme", "name": "prod" }),
    )?;
    rejects::<SetSnapshotTagRequest>(json!({ "tenant": "acme", "name": "has space", "digest": DIGEST_A }))?;
    rejects::<DeleteSnapshotTagRequest>(json!({ "tenant": "acme", "name": "" }))?;
    rejects::<SnapshotTagHistoryRequest>(json!({ "tenant": "acme", "name": "a/b" }))?;
    rejects::<SnapshotGetRequest>(json!({ "tenant": "acme", "digest": "sha256:short" }))
}

#[test]
fn lease_by_name_requests_and_snapshot_requests_round_trip() -> TestResult {
    check(
        GetLeaseByNameRequest {
            tenant: tenant()?,
            name: "build".parse()?,
        },
        json!({ "tenant": "acme", "name": "build" }),
    )?;
    let (reference, reference_json) = native_ref()?;
    check(
        NativeSnapshotDeleteRequest {
            tenant: tenant()?,
            reference,
        },
        json!({ "tenant": "acme", "reference": reference_json }),
    )
}

#[test]
fn snapshot_get_responses_are_tagged_by_outcome() -> TestResult {
    let row = SnapshotListRow {
        digest: digest(DIGEST_A)?,
        tenant: tenant()?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        created_at: at(5),
        parent: None,
    };
    check(
        SnapshotGetResponse::Found { row },
        json!({
            "outcome": "found",
            "row": {
                "digest": DIGEST_A,
                "tenant": "acme",
                "artifact_type": "sandbox_rootfs_v1",
                "created_at": time(5),
            },
        }),
    )?;
    check(SnapshotGetResponse::NotFound, json!({ "outcome": "not_found" }))?;
    let (error, error_json) = lease_denied();
    check(
        SnapshotGetResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": error_json["lease_id"] }),
    )?;
    rejects::<SnapshotGetResponse>(json!({ "outcome": "missing" }))?;
    rejects::<SnapshotGetResponse>(json!({ "outcome": "found" }))?;
    rejects::<SnapshotGetResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn lease_by_name_responses_are_tagged_by_outcome() -> TestResult {
    let row = LeaseListRow {
        lease_id: LeaseId::new("lease-1"),
        tenant: tenant()?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Paused,
        created_at: at(1),
        deadline: at(2),
        name: Some("build".parse()?),
        tags: SandboxTags::default(),
    };
    check(
        GetLeaseByNameResponse::Found { row },
        json!({
            "outcome": "found",
            "row": {
                "lease_id": "lease-1",
                "tenant": "acme",
                "provider": "mock",
                "status": "paused",
                "created_at": time(1),
                "deadline": time(2),
                "name": "build",
            },
        }),
    )?;
    check(GetLeaseByNameResponse::NotFound, json!({ "outcome": "not_found" }))?;
    let (error, _) = lease_denied();
    check(
        GetLeaseByNameResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    rejects::<GetLeaseByNameResponse>(json!({ "outcome": "missing" }))?;
    rejects::<GetLeaseByNameResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn snapshot_tree_responses_omit_an_absent_denial() -> TestResult {
    let nodes = vec![
        SnapshotTreeNode {
            digest: digest(DIGEST_A)?,
            parent: None,
            artifact_type: ArtifactType::SandboxRootfsV1,
            created_at: at(1),
        },
        SnapshotTreeNode {
            digest: digest(DIGEST_B)?,
            parent: Some(digest(DIGEST_A)?),
            artifact_type: ArtifactType::SandboxRootfsV1,
            created_at: at(2),
        },
    ];
    let nodes_json = json!([
        { "digest": DIGEST_A, "artifact_type": "sandbox_rootfs_v1", "created_at": time(1) },
        { "digest": DIGEST_B, "parent": DIGEST_A, "artifact_type": "sandbox_rootfs_v1", "created_at": time(2) },
    ]);
    check(
        SnapshotTreeResponse {
            nodes: nodes.clone(),
            denied: None,
        },
        json!({ "nodes": nodes_json }),
    )?;
    let (error, error_json) = lease_denied();
    check(
        SnapshotTreeResponse {
            nodes: Vec::new(),
            denied: Some(error),
        },
        json!({ "nodes": [], "denied": error_json }),
    )?;
    let older: SnapshotTreeResponse = serde_json::from_value(json!({ "nodes": [] }))?;
    assert_eq!(older.denied, None);
    rejects::<SnapshotTreeResponse>(json!({ "denied": null }))
}

#[test]
fn snapshot_tag_history_responses_omit_an_absent_denial() -> TestResult {
    let entries = vec![SnapshotTagAuditEntry {
        name: tag_name()?,
        digest: digest(DIGEST_A)?,
        set_at: at(9),
    }];
    check(
        SnapshotTagHistoryResponse {
            entries: entries.clone(),
            denied: None,
        },
        json!({ "entries": [{ "name": "prod", "digest": DIGEST_A, "set_at": time(9) }] }),
    )?;
    let (error, error_json) = lease_denied();
    check(
        SnapshotTagHistoryResponse {
            entries: Vec::new(),
            denied: Some(error),
        },
        json!({ "entries": [], "denied": error_json }),
    )?;
    rejects::<SnapshotTagAuditEntry>(json!({ "name": "bad name", "digest": DIGEST_A, "set_at": time(9) }))
}

#[test]
fn tag_responses_are_tagged_by_outcome() -> TestResult {
    let (error, error_json) = lease_denied();
    let denied = |outcome: &str| {
        let mut value = error_json.clone();
        value["outcome"] = json!(outcome);
        value
    };
    check(SetSnapshotTagResponse::Applied, json!({ "outcome": "applied" }))?;
    check(SetSnapshotTagResponse::Denied(error.clone()), denied("denied"))?;
    check(DeleteSnapshotTagResponse::Applied, json!({ "outcome": "applied" }))?;
    check(DeleteSnapshotTagResponse::NotFound, json!({ "outcome": "not_found" }))?;
    check(DeleteSnapshotTagResponse::Denied(error), denied("denied"))?;
    rejects::<SetSnapshotTagResponse>(json!({ "outcome": "missing" }))?;
    rejects::<SetSnapshotTagResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<DeleteSnapshotTagResponse>(json!({ "outcome": "missing" }))?;
    rejects::<DeleteSnapshotTagResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn export_requests_round_trip() -> TestResult {
    let (target, target_json) = push_target();
    check(target.clone(), target_json.clone())?;
    check(
        SnapshotExportRequest {
            lease_id: LeaseId::new("lease-1"),
            target: target.clone(),
        },
        json!({ "lease_id": "lease-1", "target": target_json }),
    )?;
    check(
        CheckpointRunningRequest {
            lease_id: LeaseId::new("lease-1"),
            target: target.clone(),
            native_name: None,
        },
        json!({ "lease_id": "lease-1", "target": target_json }),
    )?;
    check(
        CheckpointRunningRequest {
            lease_id: LeaseId::new("lease-1"),
            target,
            native_name: Some("checkpoint".to_owned()),
        },
        json!({ "lease_id": "lease-1", "target": target_json, "native_name": "checkpoint" }),
    )
}

#[test]
fn snapshot_lease_requests_encode_native_acceptance_as_a_boolean() -> TestResult {
    let accepted = SnapshotLeaseRequest {
        tenant: tenant()?,
        lease_id: LeaseId::new("lease-1"),
        accept_native: NativeSnapshotAcceptance::Accepted,
    };
    check(
        accepted,
        json!({ "tenant": "acme", "lease_id": "lease-1", "accept_native": true }),
    )?;
    let declined = SnapshotLeaseRequest {
        tenant: tenant()?,
        lease_id: LeaseId::new("lease-1"),
        accept_native: NativeSnapshotAcceptance::Declined,
    };
    check(
        declined.clone(),
        json!({ "tenant": "acme", "lease_id": "lease-1", "accept_native": false }),
    )?;
    let older: SnapshotLeaseRequest = serde_json::from_value(json!({ "tenant": "acme", "lease_id": "lease-1" }))?;
    assert_eq!(older, declined);
    rejects::<SnapshotLeaseRequest>(json!({ "tenant": "acme", "lease_id": "lease-1", "accept_native": "yes" }))
}

#[test]
fn native_snapshot_acceptance_converts_to_and_from_a_boolean() {
    assert_eq!(NativeSnapshotAcceptance::default(), NativeSnapshotAcceptance::Declined);
    assert!(NativeSnapshotAcceptance::from(true).is_accepted());
    assert!(!NativeSnapshotAcceptance::from(false).is_accepted());
    assert!(bool::from(NativeSnapshotAcceptance::Accepted));
    assert!(!bool::from(NativeSnapshotAcceptance::Declined));
}

#[test]
fn snapshot_lease_responses_are_tagged_by_outcome() -> TestResult {
    let (reference, reference_json) = oci_ref()?;
    check(
        SnapshotLeaseResponse::Created {
            receipt: LeaseSnapshotReceipt { reference, bytes: 7 },
        },
        json!({ "outcome": "created", "receipt": { "reference": reference_json, "bytes": 7 } }),
    )?;
    let (native, native_json) = native_ref()?;
    check(
        SnapshotLeaseResponse::CreatedNative {
            receipt: NativeSnapshotReceipt {
                reference: native,
                bytes: None,
            },
        },
        json!({ "outcome": "created_native", "receipt": { "reference": native_json } }),
    )?;
    check(SnapshotLeaseResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    let (error, _) = lease_denied();
    check(
        SnapshotLeaseResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    rejects::<SnapshotLeaseResponse>(json!({ "outcome": "missing" }))?;
    rejects::<SnapshotLeaseResponse>(json!({ "outcome": "created" }))?;
    rejects::<SnapshotLeaseResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn delete_responses_are_tagged_by_outcome() -> TestResult {
    check(
        SnapshotDeleteResponse::Deleted { bytes: 3 },
        json!({ "outcome": "deleted", "bytes": 3 }),
    )?;
    check(SnapshotDeleteResponse::NotFound, json!({ "outcome": "not_found" }))?;
    check(SnapshotDeleteResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    check(NativeSnapshotDeleteResponse::Deleted, json!({ "outcome": "deleted" }))?;
    check(
        NativeSnapshotDeleteResponse::NotFound,
        json!({ "outcome": "not_found" }),
    )?;
    check(
        NativeSnapshotDeleteResponse::Unsupported,
        json!({ "outcome": "unsupported" }),
    )?;
    let (error, _) = lease_denied();
    let denied = json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" });
    check(SnapshotDeleteResponse::Denied(error.clone()), denied.clone())?;
    check(NativeSnapshotDeleteResponse::Denied(error), denied)?;
    rejects::<SnapshotDeleteResponse>(json!({ "outcome": "missing" }))?;
    rejects::<SnapshotDeleteResponse>(json!({ "outcome": "deleted" }))?;
    rejects::<SnapshotDeleteResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<NativeSnapshotDeleteResponse>(json!({ "outcome": "missing" }))?;
    rejects::<NativeSnapshotDeleteResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn snapshot_export_errors_are_tagged_by_kind() -> TestResult {
    for (error, kind, display) in [
        (
            SnapshotExportError::Unsupported {
                message: "no export".to_owned(),
            },
            "unsupported",
            "snapshot export unsupported: no export",
        ),
        (
            SnapshotExportError::Transient {
                message: "retry".to_owned(),
            },
            "transient",
            "snapshot export failed transiently: retry",
        ),
        (
            SnapshotExportError::Permanent {
                message: "gone".to_owned(),
            },
            "permanent",
            "snapshot export failed permanently: gone",
        ),
    ] {
        assert_eq!(error.to_string(), display);
        let message = error.to_string();
        let expected_message = message
            .rsplit_once(": ")
            .map(|(_, message)| message.to_owned())
            .ok_or("the display has a message")?;
        check(error, json!({ "kind": kind, "message": expected_message }))?;
    }
    rejects::<SnapshotExportError>(json!({ "kind": "missing", "message": "x" }))?;
    rejects::<SnapshotExportError>(json!({ "kind": "permanent" }))
}
