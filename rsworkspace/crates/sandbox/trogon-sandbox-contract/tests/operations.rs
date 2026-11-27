use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelOperationRequest, CancelOperationResponse, CreateCancellationSupport, CreateQuotaStep,
    CreatedLeaseCompensation, GetOperationRequest, GetOperationResponse, OperationAllocation, OperationCancellation,
    OperationCapabilities, OperationCapabilitiesRequest, OperationClaim, OperationCompensationState, OperationRecord,
    OperationRequest, OperationResult, OperationState,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn spec() -> Value {
    json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    })
}

fn request() -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["mock"],
        "spec": spec(),
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
        "prewarm": false,
        "tags": {},
        "persistent": true,
    })
}

fn reservation() -> Value {
    json!({
        "tenant": "acme",
        "attempt_id": "a1",
        "created_at": time(1),
        "cpu_seconds": 60,
        "wall_seconds": 120,
        "accounting": "tenant",
    })
}

fn artifact() -> Value {
    json!({
        "registry": "ghcr.io",
        "repository": "acme/snap",
        "digest": digest(),
        "artifact_type": "sandbox_rootfs_v1",
    })
}

/// Parses `wire` and checks it serializes back to exactly the same JSON.
fn canonical<T>(wire: Value) -> Result<T, Box<dyn std::error::Error>>
where
    T: Debug + Serialize + DeserializeOwned,
{
    let parsed = serde_json::from_value::<T>(wire.clone())?;
    assert_eq!(serde_json::to_value(&parsed)?, wire);
    Ok(parsed)
}

#[test]
fn claims_cancellations_and_compensation_round_trip() -> TestResult {
    let claim = canonical::<OperationClaim>(json!({ "owner": "broker-1", "generation": 3, "renewed_at": time(5) }))?;
    assert_eq!(claim.generation, 3);
    canonical::<OperationCancellation>(json!({ "requested_at": time(6) }))?;
    assert_eq!(
        canonical::<OperationCompensationState>(json!("pending"))?,
        OperationCompensationState::Pending
    );
    assert_eq!(
        canonical::<OperationCompensationState>(json!("cleaned"))?,
        OperationCompensationState::Cleaned
    );
    Ok(())
}

#[test]
fn allocations_round_trip_with_and_without_a_target() -> TestResult {
    let bare = canonical::<OperationAllocation>(json!({
        "lease_id": "lease-1",
        "provider": "daytona",
        "sandbox_id": "sbx-1",
        "created_at": time(2),
        "reservation": reservation(),
    }))?;
    assert_eq!(bare.target, None);
    let targeted = canonical::<OperationAllocation>(json!({
        "lease_id": "lease-1",
        "provider": "daytona",
        "sandbox_id": "sbx-1",
        "created_at": time(2),
        "reservation": reservation(),
        "target": { "worker_id": "w1", "attempt_id": "a1", "epoch": 3 },
    }))?;
    assert!(targeted.target.is_some());
    Ok(())
}

#[test]
fn allocations_reject_an_empty_sandbox_id() -> TestResult {
    let error = serde_json::from_value::<OperationAllocation>(json!({
        "lease_id": "lease-1",
        "provider": "daytona",
        "sandbox_id": "",
        "created_at": time(2),
        "reservation": reservation(),
    }))
    .err()
    .ok_or("expected error")?;
    assert_eq!(error.to_string(), "sandbox id must not be empty");
    Ok(())
}

#[test]
fn quota_steps_round_trip_each_variant() -> TestResult {
    let reserved = canonical::<CreateQuotaStep>(json!({ "step": "reserved", "cpu_seconds": 5, "wall_seconds": 9 }))?;
    assert!(matches!(reserved, CreateQuotaStep::Reserved { reservation: None, .. }));
    canonical::<CreateQuotaStep>(json!({
        "step": "reserved",
        "cpu_seconds": 5,
        "wall_seconds": 9,
        "reservation": reservation(),
    }))?;
    canonical::<CreateQuotaStep>(json!({
        "step": "releasing",
        "reservation": reservation(),
        "error": { "kind": "lease_not_found", "lease_id": "lease-1" },
    }))?;
    canonical::<CreateQuotaStep>(json!({ "step": "counted" }))?;
    Ok(())
}

#[test]
fn simple_requests_round_trip() -> TestResult {
    canonical::<OperationRequest>(json!({ "kind": "pause" }))?;
    canonical::<OperationRequest>(json!({ "kind": "pause", "max_paused_seconds": 30 }))?;
    canonical::<OperationRequest>(json!({ "kind": "resume" }))?;
    canonical::<OperationRequest>(json!({ "kind": "wake" }))?;
    canonical::<OperationRequest>(json!({ "kind": "delete" }))?;
    canonical::<OperationRequest>(json!({ "kind": "rollback", "snapshot": artifact() }))?;
    canonical::<OperationRequest>(json!({ "kind": "suspend", "level": "disk", "max_suspended_seconds": 60 }))?;
    Ok(())
}

#[test]
fn a_suspend_request_defaults_its_level_and_writes_it_back() -> TestResult {
    let parsed = serde_json::from_value::<OperationRequest>(json!({ "kind": "suspend" }))?;
    assert_eq!(
        serde_json::to_value(&parsed)?,
        json!({ "kind": "suspend", "level": "auto" })
    );
    Ok(())
}

#[test]
fn create_requests_round_trip_with_and_without_dispatch() -> TestResult {
    let bare = canonical::<OperationRequest>(json!({
        "kind": "create",
        "request": request(),
        "attempt_id": "a1",
    }))?;
    assert!(matches!(
        bare,
        OperationRequest::Create {
            dispatch: None,
            quota: None,
            ..
        }
    ));
    let full = canonical::<OperationRequest>(json!({
        "kind": "create",
        "request": request(),
        "attempt_id": "a1",
        "dispatch": { "lease_id": "lease-1", "request": request() },
        "quota": { "step": "counted" },
    }))?;
    assert!(matches!(
        full,
        OperationRequest::Create {
            dispatch: Some(_),
            quota: Some(CreateQuotaStep::Counted),
            ..
        }
    ));
    Ok(())
}

#[test]
fn results_round_trip_each_variant() -> TestResult {
    canonical::<OperationResult>(json!({ "result": "granted", "lease_id": "lease-1", "provider": "mock",
        "sandbox_id": "sbx-1", "evidence_subject": "evidence.lease-1", "deadline": time(9), "created": true }))?;
    canonical::<OperationResult>(json!({
        "result": "snapshot",
        "reference": artifact(),
        "bytes": 4096,
    }))?;
    canonical::<OperationResult>(json!({ "result": "paused", "reap_at": time(1) }))?;
    canonical::<OperationResult>(json!({ "result": "resumed", "deadline": time(2) }))?;
    canonical::<OperationResult>(json!({ "result": "suspended", "level": "memory", "reap_at": time(3) }))?;
    canonical::<OperationResult>(json!({ "result": "woken", "deadline": time(4) }))?;
    canonical::<OperationResult>(json!({ "result": "stopped" }))?;
    canonical::<OperationResult>(json!({ "result": "deleted" }))?;
    canonical::<OperationResult>(json!({ "result": "rolled_back" }))?;
    canonical::<OperationResult>(json!({ "result": "unsupported" }))?;
    Ok(())
}

fn minimal_record() -> Value {
    json!({
        "id": "op-1",
        "tenant": "acme",
        "kind": "pause",
        "request_fingerprint": "abc",
        "state": "queued",
        "attempts": 0,
        "requested_at": time(1),
    })
}

#[test]
fn a_minimal_record_omits_unset_fields() -> TestResult {
    let record = canonical::<OperationRecord>(minimal_record())?;
    assert_eq!(record.state, OperationState::Queued);
    assert_eq!(record.claim, None);
    assert_eq!(record.result, None);
    Ok(())
}

#[test]
fn a_record_from_an_older_peer_defaults_its_attempts() -> TestResult {
    let mut wire = minimal_record();
    wire.as_object_mut().ok_or("object")?.remove("attempts");
    let record = serde_json::from_value::<OperationRecord>(wire)?;
    assert_eq!(record.attempts, 0);
    Ok(())
}

#[test]
fn a_full_record_round_trips_exactly() -> TestResult {
    let wire = json!({
        "id": "op-1",
        "tenant": "acme",
        "kind": "create",
        "target": "lease-1",
        "request_fingerprint": "abc",
        "request": { "kind": "create", "request": request(), "attempt_id": "a1" },
        "state": "running",
        "cancellation": { "requested_at": time(2) },
        "allocation": {
            "lease_id": "lease-1",
            "provider": "mock",
            "sandbox_id": "sbx-1",
            "created_at": time(2),
            "reservation": reservation(),
        },
        "compensation": "pending",
        "phase": "allocate",
        "attempts": 2,
        "claim": { "owner": "broker-1", "generation": 1, "renewed_at": time(3) },
        "progress": "half",
        "result": { "result": "deleted" },
        "error": { "kind": "lease_not_found", "lease_id": "lease-1" },
        "requested_at": time(1),
        "started_at": time(2),
        "completed_at": time(3),
        "deadline": time(4),
        "next_attempt_at": time(5),
        "gc_after": time(6),
    });
    let record = canonical::<OperationRecord>(wire)?;
    assert_eq!(record.attempts, 2);
    Ok(())
}

#[test]
fn lookups_and_capabilities_round_trip() -> TestResult {
    canonical::<GetOperationRequest>(json!({ "tenant": "acme", "operation_id": "op-1" }))?;
    canonical::<OperationCapabilitiesRequest>(json!({ "tenant": "acme" }))?;
    let supported = canonical::<OperationCapabilities>(json!({ "supports_create_cancellation": true }))?;
    assert_eq!(
        supported.supports_create_cancellation,
        CreateCancellationSupport::Supported
    );
    assert!(supported.supports_create_cancellation.is_supported());
    let unsupported = canonical::<OperationCapabilities>(json!({ "supports_create_cancellation": false }))?;
    assert_eq!(
        unsupported.supports_create_cancellation,
        CreateCancellationSupport::Unsupported
    );
    assert!(!unsupported.supports_create_cancellation.is_supported());
    assert_eq!(
        CreateCancellationSupport::default(),
        CreateCancellationSupport::Unsupported
    );
    Ok(())
}

#[test]
fn get_responses_round_trip_each_variant() -> TestResult {
    let found = canonical::<GetOperationResponse>(json!({ "outcome": "found", "operation": minimal_record() }))?;
    assert!(matches!(found, GetOperationResponse::Found { .. }));
    canonical::<GetOperationResponse>(json!({ "outcome": "not_found" }))?;
    canonical::<GetOperationResponse>(json!({
        "outcome": "denied",
        "kind": "lease_not_found",
        "lease_id": "lease-1",
    }))?;
    Ok(())
}

#[test]
fn cancel_requests_default_to_keeping_the_created_lease() -> TestResult {
    let parsed = serde_json::from_value::<CancelOperationRequest>(json!({ "tenant": "acme", "operation_id": "op-1" }))?;
    assert_eq!(parsed.compensate_created, CreatedLeaseCompensation::Keep);
    assert!(!parsed.compensate_created.is_compensate());
    assert_eq!(
        serde_json::to_value(&parsed)?,
        json!({ "tenant": "acme", "operation_id": "op-1", "compensate_created": false })
    );
    let compensating = canonical::<CancelOperationRequest>(json!({
        "tenant": "acme",
        "operation_id": "op-1",
        "compensate_created": true,
    }))?;
    assert_eq!(compensating.compensate_created, CreatedLeaseCompensation::Compensate);
    assert!(compensating.compensate_created.is_compensate());
    assert_eq!(CreatedLeaseCompensation::default(), CreatedLeaseCompensation::Keep);
    let explicit = canonical::<CancelOperationRequest>(json!({
        "tenant": "acme",
        "operation_id": "op-1",
        "compensate_created": false,
    }))?;
    assert_eq!(explicit, parsed);
    Ok(())
}

#[test]
fn cancel_responses_round_trip_each_variant() -> TestResult {
    canonical::<CancelOperationResponse>(json!({ "outcome": "canceled" }))?;
    canonical::<CancelOperationResponse>(json!({ "outcome": "pending" }))?;
    canonical::<CancelOperationResponse>(json!({ "outcome": "already_completed", "state": "succeeded" }))?;
    canonical::<CancelOperationResponse>(json!({ "outcome": "unsupported" }))?;
    canonical::<CancelOperationResponse>(json!({
        "outcome": "denied",
        "kind": "lease_not_found",
        "lease_id": "lease-1",
    }))?;
    Ok(())
}
