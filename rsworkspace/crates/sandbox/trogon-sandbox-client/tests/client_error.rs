use std::time::Duration;

use trogon_sandbox_client::{BusError, ClientError, TransportError};
use trogon_sandbox_contract::{DriveError, DriveId, DriveName, FileOpErrorKind, LeaseError, TenantId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_client_error_variant_has_a_classification() -> TestResult {
    let drive = DriveId {
        tenant: TenantId::new("t")?,
        name: DriveName::new("d")?,
    };
    // (error, retryable, outcome_unknown). `class()` is an exhaustive match, so a new variant cannot compile
    // without being classified; this table pins today's values.
    let table: Vec<(ClientError, bool, bool)> = vec![
        (ClientError::Denied(LeaseError::NoProviderAvailable), true, false),
        (
            ClientError::Denied(LeaseError::InvalidSpec { reason: "r".into() }),
            false,
            false,
        ),
        (ClientError::Timeout(Duration::from_secs(1)), true, true),
        (
            ClientError::Transport(TransportError::Bus(BusError::Custom("down".into()))),
            true,
            true,
        ),
        (
            ClientError::DriveDenied(DriveError::DriveNotFound { drive_id: drive }),
            false,
            false,
        ),
        (
            ClientError::DriveDenied(DriveError::StorageBackend { message: "m".into() }),
            true,
            false,
        ),
        (ClientError::FileOpUnsupported("x".into()), false, false),
        (
            ClientError::FileOpFailed {
                kind: FileOpErrorKind::Io,
                message: "m".into(),
            },
            true,
            false,
        ),
        (
            ClientError::FileOpFailed {
                kind: FileOpErrorKind::InvalidPath,
                message: "m".into(),
            },
            false,
            false,
        ),
        (ClientError::FileNotFound("/x".into()), false, false),
        (
            ClientError::EditRejected {
                occurrences: 2,
                expected: 1,
            },
            false,
            false,
        ),
    ];
    for (error, retryable, outcome_unknown) in table {
        assert_eq!(error.retryable(), retryable, "{error}");
        assert_eq!(error.outcome_unknown(), outcome_unknown, "{error}");
        assert!(!error.hint().is_empty(), "{error}");
    }
    Ok(())
}

#[test]
fn a_transport_error_converts_into_a_client_error() {
    let error: ClientError = TransportError::InvalidRequest("bad".into()).into();
    assert!(matches!(error, ClientError::Transport(_)));
    assert!(error.outcome_unknown());
}

#[test]
fn a_bus_error_converts_into_a_client_error() {
    let error: ClientError = BusError::Custom("down".into()).into();
    assert!(matches!(error, ClientError::Transport(TransportError::Bus(_))));
}
