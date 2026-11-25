use crate::constants::MAX_DATA_BYTES;
use crate::{
    Cancellation, Certainty, Delivery, Environment, ExecutionId, PlatformSupport, ProtocolError, ProtocolVersion,
    ReleaseState, SignalNumber, SignalRequestId, TimeoutState, Truncation,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub pid: u32,
    pub start_time: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    AwaitingInit,
    Held,
    Running,
    Draining,
    Absent,
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorError {
    #[error("unsupported platform")]
    UnsupportedPlatform,
    #[error("kernel prerequisite")]
    KernelPrerequisite,
    #[error("invalid frame")]
    InvalidFrame,
    #[error("invalid command")]
    InvalidCommand,
    #[error("wrong execution")]
    WrongExecution,
    #[error("wrong phase")]
    WrongPhase,
    #[error("setup failed")]
    SetupFailed,
    #[error("exec failed")]
    ExecFailed,
    #[error("identity unknown")]
    IdentityUnknown,
    #[error("process limit")]
    ProcessLimit,
    #[error("cleanup pending")]
    CleanupPending,
    #[error("io failure")]
    IoFailure,
}

pub use SupervisorError as ErrorCode;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventWire {
    Hello {
        version: u32,
        execution_id: ExecutionId,
        environment: BTreeMap<String, String>,
    },
    Ready {
        version: u32,
        execution_id: ExecutionId,
        helper: Identity,
        root: Identity,
        process_group: i32,
        session: i32,
    },
    Released {
        execution_id: ExecutionId,
    },
    SignalResult {
        execution_id: ExecutionId,
        request_id: u64,
        signal: i32,
        delivered: Delivery,
    },
    Output {
        execution_id: ExecutionId,
        stream: Stream,
        data: Vec<u8>,
    },
    RootExited {
        execution_id: ExecutionId,
        exit_code: Option<i32>,
        signal: Option<i32>,
    },
    FamilyAbsent {
        execution_id: ExecutionId,
        released: ReleaseState,
        root_exit_code: Option<i32>,
        root_signal: Option<i32>,
        timed_out: TimeoutState,
        canceled: Cancellation,
        reaped_count: u64,
        stdout_truncated: Truncation,
        stderr_truncated: Truncation,
    },
    Error {
        execution_id: ExecutionId,
        code: ErrorCode,
        uncertain: Certainty,
    },
    Status {
        execution_id: ExecutionId,
        phase: Phase,
    },
    Probe {
        version: u32,
        supported: PlatformSupport,
    },
}

pub(crate) fn validate_event(event: &EventWire) -> Result<(), ProtocolError> {
    match event {
        EventWire::Hello {
            version, environment, ..
        } => {
            ProtocolVersion::new(*version)?;
            Environment::new(environment.clone())?;
        }
        EventWire::Ready { version, .. } | EventWire::Probe { version, .. } => {
            ProtocolVersion::new(*version)?;
        }
        EventWire::SignalResult { request_id, signal, .. } => {
            SignalRequestId::new(*request_id)?;
            SignalNumber::new(*signal)?;
        }
        EventWire::Output { data, .. } if data.len() > MAX_DATA_BYTES => {
            return Err(ProtocolError::OversizedFrame);
        }
        _ => {}
    }
    Ok(())
}

impl AsRef<EventWire> for EventWire {
    fn as_ref(&self) -> &EventWire {
        self
    }
}
