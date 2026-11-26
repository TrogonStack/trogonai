use std::path::PathBuf;
use std::time::Duration;

use trogon_sandbox_client::{BusError, ClientError, TransportError};
use trogon_sandbox_contract::{DriveError, DriveId, DriveName, FileOpErrorKind, LeaseError, TenantId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn json_error() -> Result<serde_json::Error, Box<dyn std::error::Error>> {
    serde_json::from_slice::<u32>(b"x")
        .err()
        .ok_or_else(|| "expected a decode failure".into())
}

fn path() -> PathBuf {
    PathBuf::from("/tmp/a")
}

#[test]
fn every_transport_error_names_what_failed() -> TestResult {
    let table: Vec<(TransportError, &str)> = vec![
        (
            TransportError::Bus(BusError::Custom("down".into())),
            "bus: bus error: down",
        ),
        (TransportError::Encode(json_error()?), "encoding the request failed"),
        (TransportError::Decode(json_error()?), "decoding the reply failed"),
        (
            TransportError::UnexpectedReply {
                operation: "stat",
                reply: "Done".into(),
            },
            "unexpected reply to stat: Done",
        ),
        (TransportError::InvalidRequest("bad".into()), "invalid request: bad"),
        (
            TransportError::Local {
                path: path(),
                source: std::io::Error::other("boom"),
            },
            "/tmp/a failed: boom",
        ),
        (
            TransportError::SizeChanged { path: path() },
            "/tmp/a changed size while it was being transferred",
        ),
        (
            TransportError::EmptyChunk { path: path() },
            "returned no bytes for /tmp/a",
        ),
        (
            TransportError::ShorterThanExpected { path: path() },
            "/tmp/a ended before its expected size",
        ),
        (
            TransportError::IntegrityCheckFailed { path: path() },
            "/tmp/a failed its integrity check",
        ),
        (
            TransportError::PathEscapes { path: path() },
            "/tmp/a escapes the transfer root",
        ),
        (
            TransportError::NotAFile { path: path() },
            "/tmp/a is not a regular file",
        ),
        (
            TransportError::NotADirectory { path: path() },
            "/tmp/a is not a directory",
        ),
        (
            TransportError::MissingDependency {
                tool: "tar".to_string(),
            },
            "needs `tar`",
        ),
    ];
    for (error, expected) in table {
        assert!(error.to_string().contains(expected), "{error} lacks {expected}");
    }
    Ok(())
}

#[test]
fn a_bus_error_converts_into_a_transport_client_error() {
    let error: ClientError = BusError::Custom("down".into()).into();
    assert!(matches!(error, ClientError::Transport(TransportError::Bus(_))));
    let error: ClientError = TransportError::InvalidRequest("x".into()).into();
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::InvalidRequest(_))
    ));
}

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
            ClientError::DriveDenied(DriveError::DriveNotFound {
                drive_id: drive.clone(),
            }),
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
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}
