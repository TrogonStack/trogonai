use thiserror::Error;
use trogon_nats::lease::{LeaseConfigError, LeaseRenewIntervalError, LeaseTtlError};
use trogonai_session_contracts::{ContractValidationError, SessionId};

use crate::state::SessionBusyPolicy;

#[derive(Debug, Error)]
pub enum SessionKernelError {
    #[error("session {session_id} is busy processing another mutating operation")]
    SessionBusy {
        session_id: SessionId,
        policy: SessionBusyPolicy,
    },

    #[error("invalid session lease configuration: {0}")]
    LeaseConfig(#[from] LeaseConfigError),

    #[error("invalid session lease ttl: {0}")]
    LeaseTtl(#[from] LeaseTtlError),

    #[error("invalid session lease renew interval: {0}")]
    LeaseRenewInterval(#[from] LeaseRenewIntervalError),

    #[error("failed to acquire session lease for {session_id}: {detail}")]
    LeaseAcquire { session_id: SessionId, detail: String },

    #[error("failed to renew session lease for {session_id}: {detail}")]
    LeaseRenew { session_id: SessionId, detail: String },

    #[error("failed to release session lease for {session_id}: {detail}")]
    LeaseRelease { session_id: SessionId, detail: String },

    #[error("event contract validation failed: {0}")]
    ContractValidation(#[from] ContractValidationError),

    #[error("event payload exceeds max size ({actual} > {limit} bytes)")]
    EventPayloadTooLarge { actual: usize, limit: usize },

    #[error("snapshot exceeds max size ({actual} > {limit} bytes)")]
    SnapshotTooLarge { actual: usize, limit: usize },

    #[error("failed to publish session event: {0}")]
    EventPublish(String),

    #[error("failed to read session events: {0}")]
    EventRead(String),

    #[error("failed to load session snapshot: {0}")]
    SnapshotLoad(String),

    #[error("failed to store session snapshot: {0}")]
    SnapshotStore(String),

    #[error("failed to load session usage: {0}")]
    UsageLoad(String),

    #[error("failed to store session usage: {0}")]
    UsageStore(String),

    #[error("failed to provision session kernel NATS resources: {0}")]
    Provision(String),

    #[error("session event seq {expected} does not follow last seq {last}")]
    InvalidSeq { expected: u64, last: u64 },

    #[error("session {session_id} event seq mismatch: got {actual}, expected {expected}")]
    SeqMismatch {
        session_id: SessionId,
        actual: u64,
        expected: u64,
    },

    #[error("protobuf decode failed: {0}")]
    Decode(String),

    #[error("protobuf encode failed: {0}")]
    Encode(String),

    #[error("recovery failed for session {session_id}: {detail}")]
    Recovery { session_id: SessionId, detail: String },
}

impl SessionKernelError {
    pub fn session_busy(session_id: SessionId) -> Self {
        Self::SessionBusy {
            session_id,
            policy: SessionBusyPolicy::SessionBusy,
        }
    }
}
