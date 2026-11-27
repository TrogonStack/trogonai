use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LifecycleEvent, OutputStream, SandboxPhase};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
fn every_phase_is_a_snake_case_string() -> TestResult {
    let phases = [
        (SandboxPhase::Allocate, "allocate"),
        (SandboxPhase::RestoreSnapshot, "restore_snapshot"),
        (SandboxPhase::MountFilesystem, "mount_filesystem"),
        (SandboxPhase::PrepareWorkspace, "prepare_workspace"),
        (SandboxPhase::PullImage, "pull_image"),
        (SandboxPhase::PullingImage, "pulling_image"),
        (SandboxPhase::BootRuntime, "boot_runtime"),
        (SandboxPhase::Execute, "execute"),
        (SandboxPhase::Snapshotting, "snapshotting"),
        (SandboxPhase::Reclaim, "reclaim"),
    ];
    for (phase, name) in phases {
        assert_eq!(serde_json::to_value(phase)?, json!(name));
        assert_eq!(serde_json::from_value::<SandboxPhase>(json!(name))?, phase);
    }
    Ok(())
}

#[test]
fn lease_status_events_round_trip() -> TestResult {
    canonical::<LifecycleEvent>(json!({ "status": "started", "sandbox_id": "sbx-1" }))?;
    canonical::<LifecycleEvent>(json!({ "status": "completed", "sandbox_id": "sbx-1" }))?;
    canonical::<LifecycleEvent>(json!({ "status": "failed", "sandbox_id": "sbx-1", "error": "boom" }))?;
    canonical::<LifecycleEvent>(json!({ "status": "cancelled", "sandbox_id": "sbx-1" }))?;
    Ok(())
}

#[test]
fn phase_events_round_trip_with_and_without_detail() -> TestResult {
    canonical::<LifecycleEvent>(json!({ "status": "phase_started", "phase": "allocate" }))?;
    canonical::<LifecycleEvent>(json!({ "status": "phase_started", "phase": "allocate", "detail": "node-1" }))?;
    canonical::<LifecycleEvent>(json!({ "status": "phase_completed", "phase": "execute", "duration_ms": 12 }))?;
    canonical::<LifecycleEvent>(json!({
        "status": "phase_completed",
        "phase": "execute",
        "duration_ms": 12,
        "detail": "ok",
    }))?;
    canonical::<LifecycleEvent>(json!({
        "status": "phase_failed",
        "phase": "pull_image",
        "duration_ms": 5,
        "error": "denied",
    }))?;
    let failed = canonical::<LifecycleEvent>(json!({
        "status": "phase_failed",
        "phase": "pull_image",
        "duration_ms": 5,
        "error": "denied",
        "detail": "registry",
    }))?;
    assert!(matches!(failed, LifecycleEvent::PhaseFailed { detail: Some(_), .. }));
    Ok(())
}

#[test]
fn output_events_reuse_the_command_output_stream() -> TestResult {
    let stdout = canonical::<LifecycleEvent>(json!({ "status": "output", "stream": "stdout", "text": "hi" }))?;
    assert_eq!(
        stdout,
        LifecycleEvent::Output {
            stream: OutputStream::Stdout,
            text: "hi".to_owned()
        }
    );
    canonical::<LifecycleEvent>(json!({ "status": "output", "stream": "stderr", "text": "oops" }))?;
    Ok(())
}

#[test]
fn a_lifecycle_event_rejects_an_empty_sandbox_id_and_an_unknown_status() -> TestResult {
    let error = serde_json::from_value::<LifecycleEvent>(json!({ "status": "started", "sandbox_id": "" }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "sandbox id must not be empty");
    let error = serde_json::from_value::<LifecycleEvent>(json!({ "status": "paused" }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(
        error.to_string(),
        "unknown variant `paused`, expected one of `started`, `completed`, `failed`, `cancelled`, `phase_started`, `phase_completed`, `phase_failed`, `output`"
    );
    Ok(())
}
