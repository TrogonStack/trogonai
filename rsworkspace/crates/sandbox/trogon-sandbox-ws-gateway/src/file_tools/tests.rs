use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{FileOpErrorKind, FileOpResponse};

use super::file_op_outcome;
use crate::DispatchError;

#[test]
fn typed_refusals_become_outcomes() {
    let unsupported = file_op_outcome(Err(ClientError::FileOpUnsupported("no pty".to_string())));
    assert!(matches!(unsupported, Ok(FileOpResponse::Unsupported { ref message }) if message == "no pty"));
    let missing = file_op_outcome(Err(ClientError::FileNotFound("/a".to_string())));
    assert!(matches!(missing, Ok(FileOpResponse::NotFound { ref path }) if path == "/a"));
    let failed = file_op_outcome(Err(ClientError::FileOpFailed {
        kind: FileOpErrorKind::Io,
        message: "disk".to_string(),
    }));
    assert!(matches!(
        failed,
        Ok(FileOpResponse::Failed {
            kind: FileOpErrorKind::Io,
            ..
        })
    ));
}

#[test]
fn other_failures_stay_failures() {
    let timeout = file_op_outcome(Err(ClientError::Timeout(std::time::Duration::from_secs(1))));
    assert!(matches!(timeout, Err(DispatchError::Client(ref error)) if matches!(**error, ClientError::Timeout(_))));
}
