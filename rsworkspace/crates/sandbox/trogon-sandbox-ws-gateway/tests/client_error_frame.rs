mod support;

use std::time::Duration;

use serde_json::Value;
use support::TestResult;
use trogon_sandbox_client::{ClientError, TransportError};
use trogon_sandbox_contract::{DriveError, FileOpErrorKind, LeaseError, LeaseId, ProviderKind};
use trogon_sandbox_ws_gateway::{client_error_code, client_error_frame};

fn error_of(error: &ClientError) -> TestResult<Value> {
    let frame = serde_json::to_value(client_error_frame("req", error))?;
    assert_eq!(frame["id"], "req");
    Ok(frame["error"].clone())
}

#[test]
fn every_client_failure_has_a_stable_code() -> TestResult {
    let cases = [
        (
            ClientError::Denied(LeaseError::LeaseWaking {
                lease_id: LeaseId::new("lease-1"),
                retry_after_ms: 250,
            }),
            "lease_denied",
        ),
        (ClientError::Timeout(Duration::from_secs(3)), "timeout"),
        (
            ClientError::Transport(TransportError::UnexpectedReply {
                operation: "probe",
                reply: "nothing".to_string(),
            }),
            "transport",
        ),
        (
            ClientError::DriveDenied(DriveError::StorageBackend {
                message: "full".to_string(),
            }),
            "drive_denied",
        ),
        (ClientError::FileOpUnsupported("no pty".to_string()), "unsupported"),
        (
            ClientError::FileOpFailed {
                kind: FileOpErrorKind::Io,
                message: "disk".to_string(),
            },
            "file_op_failed",
        ),
        (ClientError::FileNotFound("/a".to_string()), "not_found"),
        (
            ClientError::EditRejected {
                occurrences: 2,
                expected: 1,
            },
            "edit_rejected",
        ),
    ];
    for (error, code) in cases {
        let (actual, message) = client_error_code(&error);
        assert_eq!(actual, code);
        assert!(!message.is_empty(), "{code}");
        assert_eq!(error_of(&error)?["code"], code);
    }
    Ok(())
}

#[test]
fn a_denial_carries_its_lease_provider_and_retry_delay() -> TestResult {
    let waking = error_of(&ClientError::Denied(LeaseError::LeaseWaking {
        lease_id: LeaseId::new("lease-1"),
        retry_after_ms: 250,
    }))?;
    assert_eq!(waking["lease_id"], "lease-1");
    assert_eq!(waking["retry_after_ms"], 250);
    assert!(waking["kind"].is_string());
    assert!(waking["retryable"].is_boolean());

    let rejected = error_of(&ClientError::Denied(LeaseError::ProviderRejectedDispatch {
        provider: ProviderKind::Fly,
        reason: "busy".to_string(),
    }))?;
    assert_eq!(rejected["provider"], "fly");
    assert!(rejected["retry_after_ms"].is_null());
    Ok(())
}

#[test]
fn drive_and_file_failures_carry_their_kind() -> TestResult {
    let drive = error_of(&ClientError::DriveDenied(DriveError::StorageBackend {
        message: "full".to_string(),
    }))?;
    assert_eq!(drive["kind"], "storage_backend");
    let file = error_of(&ClientError::FileOpFailed {
        kind: FileOpErrorKind::Io,
        message: "disk".to_string(),
    })?;
    assert_eq!(file["kind"], "io");
    let timeout = error_of(&ClientError::Timeout(Duration::from_secs(1)))?;
    assert!(timeout["kind"].is_null());
    assert_eq!(timeout["outcome_unknown"], true);
    Ok(())
}
