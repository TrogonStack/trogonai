use std::fmt::Debug;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, CancelOwnedLeaseResponse, CreateDriveRequest, CreateDriveResponse, DeleteDriveRequest,
    DeleteDriveResponse, DeleteLeaseRequest, DeleteLeaseResponse, DeleteNamedSandboxRequest,
    DeleteNamedSandboxResponse, DriveError, DriveId, DriveName, DriveRecord, DriveStorage, ExtendLeaseTimeoutRequest,
    ExtendLeaseTimeoutResponse, LeaseError, LeaseForkRequest, LeaseId, LeaseName, LeaseNameError, LeaseStatus,
    LeaseSummary, ListDrivesRequest, ListDrivesResponse, ListLeasesRequest, ListLeasesResponse,
    NetworkPolicyUpdateResponse, OperationId, ProviderKind, PurgeNamedRequest, PurgeNamedResponse, ReapFailure,
    ReapLeasesRequest, ReapLeasesRequestError, ReapLeasesResponse, ReapMode, SandboxId, SandboxTags, TenantId,
    UpdateLeaseTagsRequest, UpdateLeaseTagsResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("acme")?)
}

fn lease_id() -> LeaseId {
    LeaseId::new("lease-1")
}

fn lease_denied() -> (LeaseError, Value) {
    (
        LeaseError::LeaseNotFound { lease_id: lease_id() },
        json!({ "kind": "lease_not_found", "lease_id": "lease-1" }),
    )
}

fn drive_denied() -> Result<(DriveError, Value), Box<dyn std::error::Error>> {
    Ok((
        DriveError::Unauthorized {
            reason: "no access".to_owned(),
        },
        json!({ "kind": "unauthorized", "reason": "no access" }),
    ))
}

fn drive_id() -> Result<(DriveId, Value), Box<dyn std::error::Error>> {
    Ok((
        DriveId {
            tenant: tenant()?,
            name: DriveName::new("scratch")?,
        },
        json!({ "tenant": "acme", "name": "scratch" }),
    ))
}

fn drive_record() -> Result<(DriveRecord, Value), Box<dyn std::error::Error>> {
    let (id, id_json) = drive_id()?;
    Ok((
        DriveRecord {
            id,
            tenant: tenant()?,
            size_gib: 10,
            created_at: at(4),
            attached_lease: None,
            storage: DriveStorage::S3 {
                bucket: "bucket".to_owned(),
            },
        },
        json!({
            "id": id_json,
            "tenant": "acme",
            "size_gib": 10,
            "created_at": time(4),
            "attached_lease": null,
            "bucket": "bucket",
        }),
    ))
}

fn summary() -> Result<(LeaseSummary, Value), Box<dyn std::error::Error>> {
    Ok((
        LeaseSummary {
            lease_id: lease_id(),
            tenant: tenant()?,
            created_at: at(1),
            expires_at: at(2),
            status: LeaseStatus::Running,
            provider: ProviderKind::Mock,
            tags: SandboxTags::default(),
            last_activity_at: None,
        },
        json!({
            "lease_id": "lease-1",
            "tenant": "acme",
            "created_at": time(1),
            "expires_at": time(2),
            "status": "running",
            "provider": "mock",
        }),
    ))
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

#[test]
fn cancel_owned_lease_messages_round_trip_and_reject_a_bad_sandbox_id() -> TestResult {
    check(
        CancelOwnedLeaseRequest {
            tenant: tenant()?,
            lease_id: lease_id(),
            sandbox_id: SandboxId::new("sbx-1")?,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "sandbox_id": "sbx-1" }),
    )?;
    rejects::<CancelOwnedLeaseRequest>(json!({ "tenant": "acme", "lease_id": "lease-1", "sandbox_id": "" }))?;
    check(
        CancelOwnedLeaseResponse::Cleaned {
            tenant: tenant()?,
            lease_id: lease_id(),
            sandbox_id: SandboxId::new("sbx-1")?,
        },
        json!({ "outcome": "cleaned", "tenant": "acme", "lease_id": "lease-1", "sandbox_id": "sbx-1" }),
    )?;
    check(CancelOwnedLeaseResponse::NotHeld, json!({ "outcome": "not_held" }))?;
    let (error, _) = lease_denied();
    check(
        CancelOwnedLeaseResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    rejects::<CancelOwnedLeaseResponse>(json!({ "outcome": "missing" }))?;
    rejects::<CancelOwnedLeaseResponse>(json!({ "outcome": "cleaned", "tenant": "acme" }))?;
    rejects::<CancelOwnedLeaseResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn lease_fork_requests_round_trip() -> TestResult {
    check(
        LeaseForkRequest {
            tenant: tenant()?,
            source_lease_id: lease_id(),
            ttl_seconds: 600,
            idempotency_key: "key-1".to_owned(),
        },
        json!({
            "tenant": "acme",
            "source_lease_id": "lease-1",
            "ttl_seconds": 600,
            "idempotency_key": "key-1",
        }),
    )?;
    rejects::<LeaseForkRequest>(json!({ "tenant": "acme", "source_lease_id": "lease-1", "ttl_seconds": -1 }))
}

#[test]
fn network_policy_and_tag_update_responses_are_tagged_by_outcome() -> TestResult {
    let (error, _) = lease_denied();
    let denied = json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" });
    check(NetworkPolicyUpdateResponse::Applied, json!({ "outcome": "applied" }))?;
    check(NetworkPolicyUpdateResponse::Denied(error.clone()), denied.clone())?;
    check(UpdateLeaseTagsResponse::Applied, json!({ "outcome": "applied" }))?;
    check(UpdateLeaseTagsResponse::Denied(error), denied)?;
    rejects::<NetworkPolicyUpdateResponse>(json!({ "outcome": "missing" }))?;
    rejects::<NetworkPolicyUpdateResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<UpdateLeaseTagsResponse>(json!({ "outcome": "missing" }))?;
    rejects::<UpdateLeaseTagsResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn update_lease_tags_requests_round_trip_and_cap_the_tags() -> TestResult {
    check(
        UpdateLeaseTagsRequest {
            lease_id: lease_id(),
            tags: serde_json::from_value(json!({ "env": "prod" }))?,
        },
        json!({ "lease_id": "lease-1", "tags": { "env": "prod" } }),
    )?;
    rejects::<UpdateLeaseTagsRequest>(json!({
        "lease_id": "lease-1",
        "tags": { "a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6" },
    }))
}

#[test]
fn create_drive_requests_flatten_their_storage() -> TestResult {
    let (id, id_json) = drive_id()?;
    check(
        CreateDriveRequest {
            tenant: tenant()?,
            drive_id: id.clone(),
            size_gib: Some(20),
            storage: DriveStorage::S3 {
                bucket: "bucket".to_owned(),
            },
        },
        json!({ "tenant": "acme", "drive_id": id_json, "size_gib": 20, "bucket": "bucket" }),
    )?;
    check(
        CreateDriveRequest {
            tenant: tenant()?,
            drive_id: id,
            size_gib: None,
            storage: DriveStorage::S3 {
                bucket: "bucket".to_owned(),
            },
        },
        json!({ "tenant": "acme", "drive_id": id_json, "size_gib": null, "bucket": "bucket" }),
    )?;
    rejects::<CreateDriveRequest>(json!({ "tenant": "acme", "drive_id": id_json }))
}

#[test]
fn drive_responses_are_tagged_by_outcome() -> TestResult {
    let (record, record_json) = drive_record()?;
    let (error, error_json) = drive_denied()?;
    check(
        CreateDriveResponse::Created(record.clone()),
        merge(json!({ "outcome": "created" }), &record_json),
    )?;
    check(
        CreateDriveResponse::AlreadyExists(record),
        merge(json!({ "outcome": "already_exists" }), &record_json),
    )?;
    check(
        CreateDriveResponse::Denied(error.clone()),
        merge(json!({ "outcome": "denied" }), &error_json),
    )?;
    check(DeleteDriveResponse::Applied, json!({ "outcome": "applied" }))?;
    check(
        DeleteDriveResponse::Denied(error),
        merge(json!({ "outcome": "denied" }), &error_json),
    )?;
    rejects::<CreateDriveResponse>(json!({ "outcome": "missing" }))?;
    rejects::<CreateDriveResponse>(json!({ "outcome": "created" }))?;
    rejects::<CreateDriveResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<DeleteDriveResponse>(json!({ "outcome": "missing" }))?;
    rejects::<DeleteDriveResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

fn merge(mut base: Value, extra: &Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

#[test]
fn drive_requests_and_list_responses_round_trip() -> TestResult {
    check(ListDrivesRequest { tenant: tenant()? }, json!({ "tenant": "acme" }))?;
    let (id, id_json) = drive_id()?;
    check(
        DeleteDriveRequest {
            tenant: tenant()?,
            drive_id: id,
        },
        json!({ "tenant": "acme", "drive_id": id_json }),
    )?;
    let (record, record_json) = drive_record()?;
    check(
        ListDrivesResponse {
            drives: vec![record],
            denied: None,
        },
        json!({ "drives": [record_json] }),
    )?;
    let (error, error_json) = drive_denied()?;
    check(
        ListDrivesResponse {
            drives: Vec::new(),
            denied: Some(error),
        },
        json!({ "drives": [], "denied": error_json }),
    )?;
    rejects::<ListDrivesResponse>(json!({ "denied": null }))
}

#[test]
fn lease_summaries_omit_empty_tags_and_absent_activity() -> TestResult {
    let (bare, bare_json) = summary()?;
    check(bare.clone(), bare_json)?;
    let busy = LeaseSummary {
        tags: serde_json::from_value(json!({ "env": "prod" }))?,
        last_activity_at: Some(at(3)),
        ..bare
    };
    check(
        busy,
        json!({
            "lease_id": "lease-1",
            "tenant": "acme",
            "created_at": time(1),
            "expires_at": time(2),
            "status": "running",
            "provider": "mock",
            "tags": { "env": "prod" },
            "last_activity_at": time(3),
        }),
    )
}

#[test]
fn list_leases_requests_omit_unset_filters() -> TestResult {
    check(
        ListLeasesRequest {
            tenant: tenant()?,
            status_filter: None,
            cursor: None,
            limit: None,
            tags: SandboxTags::default(),
        },
        json!({ "tenant": "acme" }),
    )?;
    check(
        ListLeasesRequest {
            tenant: tenant()?,
            status_filter: Some(LeaseStatus::Running),
            cursor: Some("lease-1".to_owned()),
            limit: Some(25),
            tags: tags(&[("app", "web")])?,
        },
        json!({
            "tenant": "acme",
            "status_filter": "running",
            "cursor": "lease-1",
            "limit": 25,
            "tags": { "app": "web" },
        }),
    )?;
    rejects::<ListLeasesRequest>(json!({ "tenant": "acme", "status_filter": "gone" }))?;
    rejects::<ListLeasesRequest>(json!({
        "tenant": "acme",
        "tags": { "a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6" },
    }))
}

fn tags(entries: &[(&str, &str)]) -> Result<SandboxTags, Box<dyn std::error::Error>> {
    Ok(SandboxTags::new(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    )?)
}

#[test]
fn list_leases_responses_omit_an_absent_cursor_and_denial() -> TestResult {
    let (lease, lease_json) = summary()?;
    check(
        ListLeasesResponse {
            leases: vec![lease.clone()],
            next_cursor: None,
            denied: None,
        },
        json!({ "leases": [lease_json] }),
    )?;
    let (error, error_json) = lease_denied();
    check(
        ListLeasesResponse {
            leases: vec![lease],
            next_cursor: Some("lease-1".to_owned()),
            denied: Some(error),
        },
        json!({ "leases": [summary()?.1], "next_cursor": "lease-1", "denied": error_json }),
    )?;
    rejects::<ListLeasesResponse>(json!({ "next_cursor": "x" }))
}

#[test]
fn reap_leases_requests_need_a_filter() -> TestResult {
    let request = ReapLeasesRequest::new(tenant()?, SandboxTags::default(), Some(60), None, ReapMode::Release)?;
    assert_eq!(request.tenant().as_str(), "acme");
    assert!(request.tags().is_empty());
    assert_eq!(request.older_than_seconds(), Some(60));
    assert_eq!(request.idle_for_seconds(), None);
    assert_eq!(request.dry_run(), ReapMode::Release);
    check(
        request,
        json!({ "tenant": "acme", "older_than_seconds": 60, "dry_run": false }),
    )?;
    check(
        ReapLeasesRequest::new(tenant()?, SandboxTags::default(), None, Some(30), ReapMode::DryRun)?,
        json!({ "tenant": "acme", "idle_for_seconds": 30, "dry_run": true }),
    )?;
    check(
        ReapLeasesRequest::new(tenant()?, tags(&[("app", "web")])?, None, None, ReapMode::Release)?,
        json!({ "tenant": "acme", "tags": { "app": "web" }, "dry_run": false }),
    )?;
    let defaulted: ReapLeasesRequest = serde_json::from_value(json!({ "tenant": "acme", "idle_for_seconds": 1 }))?;
    assert_eq!(defaulted.dry_run(), ReapMode::Release);
    Ok(())
}

#[test]
fn reap_leases_requests_without_a_filter_are_rejected() -> TestResult {
    let error = ReapLeasesRequest::new(tenant()?, SandboxTags::default(), None, None, ReapMode::DryRun)
        .err()
        .ok_or("an unfiltered reap should be rejected")?;
    assert_eq!(error, ReapLeasesRequestError::NoFilter);
    assert_eq!(
        error.to_string(),
        "a reap request needs at least one of tags, older_than_seconds, or idle_for_seconds"
    );
    rejects::<ReapLeasesRequest>(json!({ "tenant": "acme" }))?;
    rejects::<ReapLeasesRequest>(json!({ "tenant": "acme", "tags": {}, "dry_run": true }))?;
    rejects::<ReapLeasesRequest>(json!({ "tenant": "acme", "older_than_seconds": 1, "dry_run": "yes" }))?;
    rejects::<ReapLeasesRequest>(json!({
        "tenant": "acme",
        "tags": { "a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6" },
    }))
}

#[test]
fn reap_mode_converts_to_and_from_a_boolean() {
    assert_eq!(ReapMode::default(), ReapMode::Release);
    assert!(ReapMode::from(true).is_dry_run());
    assert!(!ReapMode::from(false).is_dry_run());
    assert!(bool::from(ReapMode::DryRun));
    assert!(!bool::from(ReapMode::Release));
}

#[test]
fn reap_leases_responses_are_tagged_by_outcome() -> TestResult {
    let (lease, lease_json) = summary()?;
    check(
        ReapLeasesResponse::Reaped {
            dry_run: ReapMode::DryRun,
            matched: vec![lease],
            released: vec![LeaseId::new("lease-2")],
            failed: vec![ReapFailure {
                lease_id: LeaseId::new("lease-3"),
                message: "boom".to_owned(),
            }],
        },
        json!({
            "outcome": "reaped",
            "dry_run": true,
            "matched": [lease_json],
            "released": ["lease-2"],
            "failed": [{ "lease_id": "lease-3", "message": "boom" }],
        }),
    )?;
    let (error, _) = lease_denied();
    check(
        ReapLeasesResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    rejects::<ReapLeasesResponse>(json!({ "outcome": "missing" }))?;
    rejects::<ReapLeasesResponse>(json!({ "outcome": "reaped" }))?;
    rejects::<ReapLeasesResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<ReapFailure>(json!({ "lease_id": "lease-3" }))
}

#[test]
fn extend_lease_timeout_messages_round_trip() -> TestResult {
    check(
        ExtendLeaseTimeoutRequest {
            lease_id: lease_id(),
            additional_seconds: 300,
        },
        json!({ "lease_id": "lease-1", "additional_seconds": 300 }),
    )?;
    check(
        ExtendLeaseTimeoutResponse::Extended { new_deadline: at(8) },
        json!({ "outcome": "extended", "new_deadline": time(8) }),
    )?;
    check(
        ExtendLeaseTimeoutResponse::Denied(LeaseError::WouldExceedMaxLifetime { max: 86400 }),
        json!({ "outcome": "denied", "kind": "would_exceed_max_lifetime", "max": 86400 }),
    )?;
    rejects::<ExtendLeaseTimeoutResponse>(json!({ "outcome": "missing" }))?;
    rejects::<ExtendLeaseTimeoutResponse>(json!({ "outcome": "extended" }))?;
    rejects::<ExtendLeaseTimeoutResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn delete_lease_messages_omit_an_absent_operation_id() -> TestResult {
    check(
        DeleteLeaseRequest {
            tenant: tenant()?,
            lease_id: lease_id(),
            operation_id: None,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1" }),
    )?;
    check(
        DeleteLeaseRequest {
            tenant: tenant()?,
            lease_id: lease_id(),
            operation_id: Some(OperationId::new("op-1")?),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "operation_id": "op-1" }),
    )?;
    rejects::<DeleteLeaseRequest>(json!({ "tenant": "acme", "lease_id": "lease-1", "operation_id": "bad id" }))?;
    check(
        DeleteLeaseResponse::Deleted { operation_id: None },
        json!({ "outcome": "deleted" }),
    )?;
    check(
        DeleteLeaseResponse::Deleted {
            operation_id: Some(OperationId::new("op-1")?),
        },
        json!({ "outcome": "deleted", "operation_id": "op-1" }),
    )?;
    let (error, _) = lease_denied();
    check(
        DeleteLeaseResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    rejects::<DeleteLeaseResponse>(json!({ "outcome": "missing" }))?;
    rejects::<DeleteLeaseResponse>(json!({ "outcome": "deleted", "operation_id": "bad id" }))?;
    rejects::<DeleteLeaseResponse>(json!({ "outcome": "denied", "kind": "unknown" }))
}

#[test]
fn named_sandbox_messages_round_trip() -> TestResult {
    check(
        DeleteNamedSandboxRequest {
            tenant: tenant()?,
            name: "build".parse()?,
        },
        json!({ "tenant": "acme", "name": "build" }),
    )?;
    check(
        PurgeNamedRequest {
            tenant: tenant()?,
            name: "build".parse()?,
        },
        json!({ "tenant": "acme", "name": "build" }),
    )?;
    check(DeleteNamedSandboxResponse::Deleted, json!({ "outcome": "deleted" }))?;
    check(DeleteNamedSandboxResponse::NotFound, json!({ "outcome": "not_found" }))?;
    let (error, _) = lease_denied();
    check(
        DeleteNamedSandboxResponse::Denied(error),
        json!({ "outcome": "denied", "kind": "lease_not_found", "lease_id": "lease-1" }),
    )?;
    check(PurgeNamedResponse::Purged, json!({ "outcome": "purged" }))?;
    check(PurgeNamedResponse::NotFound, json!({ "outcome": "not_found" }))?;
    check(
        PurgeNamedResponse::Failed {
            message: "boom".to_owned(),
        },
        json!({ "outcome": "failed", "message": "boom" }),
    )?;
    rejects::<DeleteNamedSandboxResponse>(json!({ "outcome": "missing" }))?;
    rejects::<DeleteNamedSandboxResponse>(json!({ "outcome": "denied", "kind": "unknown" }))?;
    rejects::<PurgeNamedResponse>(json!({ "outcome": "missing" }))?;
    rejects::<PurgeNamedResponse>(json!({ "outcome": "failed" }))
}

#[test]
fn lease_names_enforce_their_byte_bounds() -> TestResult {
    let longest = "x".repeat(LeaseName::MAX_LEN);
    assert_eq!(LeaseName::new(longest.clone())?.as_str(), longest);
    assert_eq!("my sandbox/1".parse::<LeaseName>()?.to_string(), "my sandbox/1");
    assert_eq!(String::from(LeaseName::try_from("build".to_owned())?), "build");

    let empty = LeaseName::new("").err().ok_or("an empty name should be rejected")?;
    assert_eq!(empty, LeaseNameError::Empty);
    assert_eq!(empty.to_string(), "lease name must not be empty");

    let long = "x".repeat(LeaseName::MAX_LEN + 1);
    let too_long = long
        .parse::<LeaseName>()
        .err()
        .ok_or("a long name should be rejected")?;
    assert_eq!(too_long, LeaseNameError::TooLong { max: 128 });
    assert_eq!(too_long.to_string(), "lease name must be at most 128 bytes");
    Ok(())
}

#[test]
fn lease_name_fields_reject_bad_names_on_the_wire() -> TestResult {
    rejects::<DeleteNamedSandboxRequest>(json!({ "tenant": "acme", "name": "" }))?;
    rejects::<PurgeNamedRequest>(json!({ "tenant": "acme", "name": "x".repeat(129) }))?;
    rejects::<DeleteNamedSandboxRequest>(json!({ "tenant": "acme", "name": 7 }))
}
