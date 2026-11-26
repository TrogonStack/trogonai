use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    ArtifactType, CancelIntent, CancelLeaseResponse, Digest, DormancyArtifact, DormancyRecord, LeaseError, LeaseId,
    OciArtifactRef, OperationId, PauseLeaseRequest, PauseLeaseResponse, ResumeLeaseRequest, ResumeLeaseResponse,
    RollbackLeaseRequest, RollbackLeaseResponse, SandboxFreezeRequest, SandboxFreezeResponse, SandboxRollbackRequest,
    SandboxRollbackResponse, SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest, SandboxWakeResponse,
    StopLeaseRequest, StopLeaseResponse, SuspendLeaseRequest, SuspendLeaseResponse, SuspendLevel, SuspendRequestLevel,
    TenantId, WakeLeaseRequest, WakeLeaseResponse, WorkerId,
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

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn operation() -> Result<OperationId, Box<dyn std::error::Error>> {
    Ok(OperationId::new("op-1")?)
}

fn artifact() -> Result<OciArtifactRef, Box<dyn std::error::Error>> {
    Ok(OciArtifactRef {
        registry: "ghcr.io".to_owned(),
        repository: "snapshots/acme".to_owned(),
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

fn artifact_json() -> Value {
    json!({
        "registry": "ghcr.io",
        "repository": "snapshots/acme",
        "digest": format!("sha256:{}", "a".repeat(64)),
        "artifact_type": "sandbox_rootfs_v1",
    })
}

fn denied() -> LeaseError {
    LeaseError::NoProviderAvailable
}

fn denied_json(outcome: &str) -> Value {
    json!({ "outcome": outcome, "kind": "no_provider_available" })
}

fn roundtrip<T>(value: T, wire: Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(&value)?, wire);
    assert_eq!(serde_json::from_value::<T>(wire)?, value);
    Ok(())
}

#[test]
fn cancel_intents_use_snake_case_names_and_default_to_release() -> TestResult {
    roundtrip(CancelIntent::Release, json!("release"))?;
    roundtrip(CancelIntent::Purge, json!("purge"))?;
    assert_eq!(CancelIntent::default(), CancelIntent::Release);
    assert_eq!(
        serde_json::from_value::<CancelIntent>(json!("delete"))
            .err()
            .ok_or("expected an error")?
            .classify(),
        Data
    );
    Ok(())
}

#[test]
fn stop_and_cancel_replies_are_tagged_by_outcome() -> TestResult {
    roundtrip(StopLeaseResponse::Stopped, json!({ "outcome": "stopped" }))?;
    roundtrip(StopLeaseResponse::Denied(denied()), denied_json("denied"))?;
    roundtrip(CancelLeaseResponse::Cancelled, json!({ "outcome": "cancelled" }))?;
    roundtrip(CancelLeaseResponse::Denied(denied()), denied_json("denied"))?;
    roundtrip(
        StopLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1" }),
    )
}

#[test]
fn pause_requests_omit_unset_options() -> TestResult {
    roundtrip(
        PauseLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            max_paused_seconds: None,
            operation_id: None,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1" }),
    )?;
    roundtrip(
        PauseLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            max_paused_seconds: Some(60),
            operation_id: Some(operation()?),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "max_paused_seconds": 60, "operation_id": "op-1" }),
    )
}

#[test]
fn pause_replies_are_tagged_by_outcome() -> TestResult {
    roundtrip(
        PauseLeaseResponse::Paused {
            reap_at: at(10),
            operation_id: None,
        },
        json!({ "outcome": "paused", "reap_at": time(10) }),
    )?;
    roundtrip(
        PauseLeaseResponse::Paused {
            reap_at: at(10),
            operation_id: Some(operation()?),
        },
        json!({ "outcome": "paused", "reap_at": time(10), "operation_id": "op-1" }),
    )?;
    roundtrip(PauseLeaseResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(PauseLeaseResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn resume_messages_roundtrip() -> TestResult {
    roundtrip(
        ResumeLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            operation_id: None,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1" }),
    )?;
    roundtrip(
        ResumeLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            operation_id: Some(operation()?),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "operation_id": "op-1" }),
    )?;
    roundtrip(
        ResumeLeaseResponse::Resumed {
            deadline: at(20),
            operation_id: None,
        },
        json!({ "outcome": "resumed", "deadline": time(20) }),
    )?;
    roundtrip(
        ResumeLeaseResponse::Resumed {
            deadline: at(20),
            operation_id: Some(operation()?),
        },
        json!({ "outcome": "resumed", "deadline": time(20), "operation_id": "op-1" }),
    )?;
    roundtrip(ResumeLeaseResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(ResumeLeaseResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn freeze_messages_roundtrip() -> TestResult {
    roundtrip(
        SandboxFreezeRequest { lease_id: lease() },
        json!({ "lease_id": "lease-1" }),
    )?;
    roundtrip(SandboxFreezeResponse::Done, json!({ "outcome": "done" }))?;
    roundtrip(SandboxFreezeResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(SandboxFreezeResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn rollback_messages_roundtrip() -> TestResult {
    roundtrip(
        RollbackLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            snapshot: artifact()?,
            operation_id: None,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "snapshot": artifact_json() }),
    )?;
    roundtrip(
        RollbackLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            snapshot: artifact()?,
            operation_id: Some(operation()?),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "snapshot": artifact_json(), "operation_id": "op-1" }),
    )?;
    roundtrip(
        RollbackLeaseResponse::RolledBack { operation_id: None },
        json!({ "outcome": "rolled_back" }),
    )?;
    roundtrip(
        RollbackLeaseResponse::RolledBack {
            operation_id: Some(operation()?),
        },
        json!({ "outcome": "rolled_back", "operation_id": "op-1" }),
    )?;
    roundtrip(RollbackLeaseResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(RollbackLeaseResponse::Denied(denied()), denied_json("denied"))?;
    roundtrip(
        SandboxRollbackRequest {
            lease_id: lease(),
            snapshot: artifact()?,
        },
        json!({ "lease_id": "lease-1", "snapshot": artifact_json() }),
    )?;
    roundtrip(SandboxRollbackResponse::Done, json!({ "outcome": "done" }))?;
    roundtrip(
        SandboxRollbackResponse::Unsupported,
        json!({ "outcome": "unsupported" }),
    )?;
    roundtrip(SandboxRollbackResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn suspend_requests_default_to_the_auto_level() -> TestResult {
    let request = serde_json::from_value::<SuspendLeaseRequest>(json!({ "tenant": "acme", "lease_id": "lease-1" }))?;
    assert_eq!(request.level, SuspendRequestLevel::Auto);
    roundtrip(
        request,
        json!({ "tenant": "acme", "lease_id": "lease-1", "level": "auto" }),
    )?;
    roundtrip(
        SuspendLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            level: SuspendRequestLevel::Disk,
            max_suspended_seconds: Some(120),
            operation_id: Some(operation()?),
        },
        json!({
            "tenant": "acme",
            "lease_id": "lease-1",
            "level": "disk",
            "max_suspended_seconds": 120,
            "operation_id": "op-1",
        }),
    )
}

#[test]
fn suspend_and_wake_replies_roundtrip() -> TestResult {
    roundtrip(
        SuspendLeaseResponse::Suspended {
            level: SuspendLevel::Memory,
            reap_at: at(30),
            operation_id: None,
        },
        json!({ "outcome": "suspended", "level": "memory", "reap_at": time(30) }),
    )?;
    roundtrip(
        SuspendLeaseResponse::Suspended {
            level: SuspendLevel::Disk,
            reap_at: at(30),
            operation_id: Some(operation()?),
        },
        json!({ "outcome": "suspended", "level": "disk", "reap_at": time(30), "operation_id": "op-1" }),
    )?;
    roundtrip(SuspendLeaseResponse::Denied(denied()), denied_json("denied"))?;
    roundtrip(
        WakeLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            operation_id: None,
        },
        json!({ "tenant": "acme", "lease_id": "lease-1" }),
    )?;
    roundtrip(
        WakeLeaseRequest {
            tenant: tenant()?,
            lease_id: lease(),
            operation_id: Some(operation()?),
        },
        json!({ "tenant": "acme", "lease_id": "lease-1", "operation_id": "op-1" }),
    )?;
    roundtrip(
        WakeLeaseResponse::Woken {
            deadline: at(40),
            operation_id: None,
        },
        json!({ "outcome": "woken", "deadline": time(40) }),
    )?;
    roundtrip(
        WakeLeaseResponse::Woken {
            deadline: at(40),
            operation_id: Some(operation()?),
        },
        json!({ "outcome": "woken", "deadline": time(40), "operation_id": "op-1" }),
    )?;
    roundtrip(WakeLeaseResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn dormancy_artifacts_are_tagged_by_kind() -> TestResult {
    roundtrip(
        DormancyArtifact::Local {
            worker_id: WorkerId::new("worker-1")?,
            id: "state-1".to_owned(),
        },
        json!({ "kind": "local", "worker_id": "worker-1", "id": "state-1" }),
    )?;
    let mut remote = artifact_json();
    remote["kind"] = json!("remote");
    roundtrip(DormancyArtifact::Remote(artifact()?), remote)?;
    let error = serde_json::from_value::<DormancyArtifact>(json!({ "kind": "elsewhere" }))
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn dormancy_records_omit_unset_fields_and_zero_failures() -> TestResult {
    roundtrip(
        DormancyRecord {
            level: SuspendLevel::Memory,
            generation: 1,
            artifact: None,
            artifact_bytes: None,
            suspended_at: at(50),
            run_remaining_seconds: 300,
            wake_failures: 0,
            woken_at: None,
        },
        json!({
            "level": "memory",
            "generation": 1,
            "suspended_at": time(50),
            "run_remaining_seconds": 300,
        }),
    )?;
    roundtrip(
        DormancyRecord {
            level: SuspendLevel::Disk,
            generation: 2,
            artifact: Some(DormancyArtifact::Local {
                worker_id: WorkerId::new("worker-1")?,
                id: "state-1".to_owned(),
            }),
            artifact_bytes: Some(4096),
            suspended_at: at(50),
            run_remaining_seconds: 300,
            wake_failures: 2,
            woken_at: Some(at(60)),
        },
        json!({
            "level": "disk",
            "generation": 2,
            "artifact": { "kind": "local", "worker_id": "worker-1", "id": "state-1" },
            "artifact_bytes": 4096,
            "suspended_at": time(50),
            "run_remaining_seconds": 300,
            "wake_failures": 2,
            "woken_at": time(60),
        }),
    )
}

#[test]
fn sandbox_suspend_and_wake_messages_roundtrip() -> TestResult {
    roundtrip(
        SandboxSuspendRequest {
            lease_id: lease(),
            generation: 3,
            level: SuspendLevel::Disk,
        },
        json!({ "lease_id": "lease-1", "generation": 3, "level": "disk" }),
    )?;
    roundtrip(
        SandboxSuspendResponse::Done {
            artifact: None,
            artifact_bytes: None,
        },
        json!({ "outcome": "done" }),
    )?;
    roundtrip(
        SandboxSuspendResponse::Done {
            artifact: Some(DormancyArtifact::Remote(artifact()?)),
            artifact_bytes: Some(8),
        },
        {
            let mut remote = artifact_json();
            remote["kind"] = json!("remote");
            json!({ "outcome": "done", "artifact": remote, "artifact_bytes": 8 })
        },
    )?;
    roundtrip(SandboxSuspendResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(SandboxSuspendResponse::Denied(denied()), denied_json("denied"))?;
    roundtrip(
        SandboxWakeRequest {
            lease_id: lease(),
            generation: 3,
            level: SuspendLevel::Memory,
            artifact: None,
        },
        json!({ "lease_id": "lease-1", "generation": 3, "level": "memory" }),
    )?;
    roundtrip(
        SandboxWakeRequest {
            lease_id: lease(),
            generation: 3,
            level: SuspendLevel::Memory,
            artifact: Some(DormancyArtifact::Local {
                worker_id: WorkerId::new("worker-1")?,
                id: "state-1".to_owned(),
            }),
        },
        json!({
            "lease_id": "lease-1",
            "generation": 3,
            "level": "memory",
            "artifact": { "kind": "local", "worker_id": "worker-1", "id": "state-1" },
        }),
    )?;
    roundtrip(SandboxWakeResponse::Done, json!({ "outcome": "done" }))?;
    roundtrip(SandboxWakeResponse::Unsupported, json!({ "outcome": "unsupported" }))?;
    roundtrip(SandboxWakeResponse::Denied(denied()), denied_json("denied"))
}

#[test]
fn unknown_outcomes_are_data_errors() -> TestResult {
    let wire = json!({ "outcome": "exploded" });
    for category in [
        serde_json::from_value::<StopLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<CancelLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<PauseLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<ResumeLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<SandboxFreezeResponse>(wire.clone()).err(),
        serde_json::from_value::<RollbackLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<SandboxRollbackResponse>(wire.clone()).err(),
        serde_json::from_value::<SuspendLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<WakeLeaseResponse>(wire.clone()).err(),
        serde_json::from_value::<SandboxSuspendResponse>(wire.clone()).err(),
        serde_json::from_value::<SandboxWakeResponse>(wire).err(),
    ] {
        assert_eq!(category.ok_or("expected an error")?.classify(), Data);
    }
    Ok(())
}

#[test]
fn a_pause_request_rejects_an_invalid_operation_id() -> TestResult {
    let error = serde_json::from_value::<PauseLeaseRequest>(
        json!({ "tenant": "acme", "lease_id": "lease-1", "operation_id": "bad id" }),
    )
    .err()
    .ok_or("expected an error")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}
