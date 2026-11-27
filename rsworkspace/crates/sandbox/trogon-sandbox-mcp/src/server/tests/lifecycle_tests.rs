use std::time::{Duration, SystemTime};

use serde_json::json;
use trogon_sandbox_contract::{
    DeleteLeaseResponse, LeaseError, NetworkPolicyUpdateResponse, PauseLeaseResponse, ResumeLeaseResponse,
    SnapshotDeleteResponse, SnapshotLeaseResponse, StopLeaseResponse, SuspendLeaseResponse, SuspendLevel,
    WakeLeaseResponse,
};

use super::support::call;
use crate::testing::TestResult;

const DIGEST: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn later() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
}

#[tokio::test]
async fn deleting_and_stopping_acknowledge() -> TestResult {
    let deleted = call(
        "sandbox_delete_lease",
        json!({"lease_id": "l1"}),
        &DeleteLeaseResponse::Deleted { operation_id: None },
    )
    .await?;
    assert!(!deleted.is_error());
    let stopped = call(
        "sandbox_stop_lease",
        json!({"lease_id": "l1"}),
        &StopLeaseResponse::Stopped,
    )
    .await?;
    assert!(!stopped.is_error());
    let refused = call(
        "sandbox_stop_lease",
        json!({"lease_id": "l1"}),
        &StopLeaseResponse::Denied(LeaseError::NoProviderAvailable),
    )
    .await?;
    assert!(refused.is_error());
    Ok(())
}

#[tokio::test]
async fn pausing_and_resuming_report_support() -> TestResult {
    let paused = call(
        "sandbox_pause_lease",
        json!({"lease_id": "l1", "max_paused_seconds": 60}),
        &PauseLeaseResponse::Paused {
            reap_at: later(),
            operation_id: None,
        },
    )
    .await?;
    assert!(paused.text()?.contains("\"paused\""));
    let unsupported = call(
        "sandbox_pause_lease",
        json!({"lease_id": "l1"}),
        &PauseLeaseResponse::Unsupported,
    )
    .await?;
    assert!(unsupported.text()?.contains("unsupported"));

    let resumed = call(
        "sandbox_resume_lease",
        json!({"lease_id": "l1"}),
        &ResumeLeaseResponse::Resumed {
            deadline: later(),
            operation_id: None,
        },
    )
    .await?;
    assert!(resumed.text()?.contains("\"resumed\""));
    let cannot = call(
        "sandbox_resume_lease",
        json!({"lease_id": "l1"}),
        &ResumeLeaseResponse::Unsupported,
    )
    .await?;
    assert!(cannot.text()?.contains("unsupported"));
    Ok(())
}

#[tokio::test]
async fn suspending_and_waking_name_the_level_and_deadline() -> TestResult {
    let reply = SuspendLeaseResponse::Suspended {
        level: SuspendLevel::Disk,
        reap_at: later(),
        operation_id: None,
    };
    let suspended = call(
        "sandbox_suspend_lease",
        json!({"lease_id": "l1", "level": "disk", "max_suspended_seconds": 5}),
        &reply,
    )
    .await?;
    assert!(suspended.text()?.contains("\"disk\""));
    let bad_level = call(
        "sandbox_suspend_lease",
        json!({"lease_id": "l1", "level": "deep"}),
        &reply,
    )
    .await?;
    assert!(bad_level.is_error());
    assert!(bad_level.sent.is_empty());
    let refused = call(
        "sandbox_suspend_lease",
        json!({"lease_id": "l1"}),
        &SuspendLeaseResponse::Denied(LeaseError::NoProviderAvailable),
    )
    .await?;
    assert!(refused.is_error());

    let woken = call(
        "sandbox_wake_lease",
        json!({"lease_id": "l1"}),
        &WakeLeaseResponse::Woken {
            deadline: later(),
            operation_id: None,
        },
    )
    .await?;
    assert!(woken.text()?.contains("\"woken\""));
    let not_woken = call(
        "sandbox_wake_lease",
        json!({"lease_id": "l1"}),
        &WakeLeaseResponse::Denied(LeaseError::NoProviderAvailable),
    )
    .await?;
    assert!(not_woken.is_error());
    Ok(())
}

#[tokio::test]
async fn snapshots_are_taken_and_deleted_by_digest() -> TestResult {
    let taken = call(
        "sandbox_snapshot_lease",
        json!({"lease_id": "l1"}),
        &SnapshotLeaseResponse::Unsupported,
    )
    .await?;
    assert!(taken.text()?.contains("unsupported"));
    let deleted = call(
        "sandbox_snapshot_delete",
        json!({"digest": DIGEST}),
        &SnapshotDeleteResponse::Deleted { bytes: 7 },
    )
    .await?;
    assert!(deleted.text()?.contains("deleted"));
    let body = deleted.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(body["tenant"], json!("acme"));
    let bad = call(
        "sandbox_snapshot_delete",
        json!({"digest": "nope"}),
        &SnapshotDeleteResponse::NotFound,
    )
    .await?;
    assert!(bad.is_error());
    assert!(bad.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_network_policy_is_parsed_before_it_is_sent() -> TestResult {
    let applied = NetworkPolicyUpdateResponse::Applied;
    let ok = call(
        "sandbox_update_network_policy",
        json!({"lease_id": "l1", "network_policy": "deny_all"}),
        &applied,
    )
    .await?;
    assert!(!ok.is_error());
    let bad = call(
        "sandbox_update_network_policy",
        json!({"lease_id": "l1", "network_policy": "open_bar"}),
        &applied,
    )
    .await?;
    assert!(bad.is_error());
    assert!(bad.sent.is_empty());
    Ok(())
}
