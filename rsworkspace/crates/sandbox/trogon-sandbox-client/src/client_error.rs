use std::time::Duration;

use trogon_sandbox_contract::{DriveError, FileOpErrorKind, LeaseError};
use trogon_sandbox_protocol::{Classified, ErrorClass};

use crate::TransportError;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("broker denied lease: {0:?}")]
    Denied(LeaseError),
    #[error("broker timeout after {0:?}")]
    Timeout(Duration),
    #[error("transport: {0}")]
    Transport(#[source] TransportError),
    #[error("broker denied drive operation: {0:?}")]
    DriveDenied(DriveError),
    #[error("file operation not supported: {0}")]
    FileOpUnsupported(String),
    #[error("file operation failed ({kind:?}): {message}")]
    FileOpFailed { kind: FileOpErrorKind, message: String },
    #[error("no such file or directory: {0}")]
    FileNotFound(String),
    #[error("atomic edit rejected: found {occurrences} occurrence(s), expected {expected}")]
    EditRejected { occurrences: u32, expected: u32 },
}

impl ClientError {
    /// How a caller should react: whether a retry may succeed, whether the request may already have been applied,
    /// and a one-line hint.
    ///
    /// A broker reply timeout and a transport failure both leave the request possibly applied (the broker may have
    /// acted before the reply was lost), so they report `outcome_unknown`: retry with the same idempotency key or
    /// check state first.
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Denied(error) => error.class(),
            Self::Timeout(_) => ErrorClass::unknown_outcome(
                "no reply before the deadline; the request may have been applied, so retry with the same idempotency key or check state first",
            ),
            Self::Transport(_) => ErrorClass::unknown_outcome(
                "the connection to the broker failed; the request may have been applied, so retry with the same idempotency key or check state first",
            ),
            Self::DriveDenied(error) => error.class(),
            Self::FileOpUnsupported(_) => {
                ErrorClass::permanent("the provider does not support this file operation; use a provider that does")
            }
            Self::FileOpFailed { kind, .. } => kind.class(),
            Self::FileNotFound(_) => ErrorClass::permanent("the path does not exist"),
            Self::EditRejected { .. } => {
                ErrorClass::permanent("the edit target did not match the expected occurrence count; re-read the file")
            }
        }
    }

    /// True when repeating the request after a backoff may succeed.
    pub fn retryable(&self) -> bool {
        self.class().retryable
    }

    /// True when the original request may already have been applied.
    pub fn outcome_unknown(&self) -> bool {
        self.class().outcome_unknown
    }

    /// One-line remediation advice.
    pub fn hint(&self) -> &'static str {
        self.class().hint
    }
}

impl From<TransportError> for ClientError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<crate::BusError> for ClientError {
    fn from(error: crate::BusError) -> Self {
        Self::Transport(TransportError::Bus(error))
    }
}
