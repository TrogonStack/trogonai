use crate::Environment;
pub use crate::constants::*;
pub use crate::{Cancellation, Certainty, Delivery, PlatformSupport, ReleaseState, TimeoutState, Truncation};
pub use crate::{Command, CommandWire, ExecutionId, ProtocolError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

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

pub fn decode_command(bytes: &[u8]) -> Result<Command, ProtocolError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame);
    }
    let wire: CommandWire = serde_json::from_slice(bytes).map_err(ProtocolError::InvalidJson)?;
    wire.try_into()
}

pub fn decode_event(bytes: &[u8]) -> Result<Event, ProtocolError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame);
    }
    let event: Event = serde_json::from_slice(bytes).map_err(ProtocolError::InvalidJson)?;
    match &event {
        Event::Hello { environment, .. } => {
            Environment::new(environment.clone())?;
        }
        Event::Output { data, .. } if data.len() > MAX_DATA_BYTES => {
            return Err(ProtocolError::OversizedFrame);
        }
        _ => {}
    }
    Ok(event)
}

pub fn encode_event(event: &Event) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = serde_json::to_vec(event).map_err(ProtocolError::InvalidJson)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn write_command(writer: &mut impl Write, command: &CommandWire) -> Result<(), ProtocolError> {
    Command::try_from(command.clone())?;
    let mut bytes = serde_json::to_vec(command).map_err(ProtocolError::InvalidJson)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame);
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).map_err(ProtocolError::Io)?;
    writer.flush().map_err(ProtocolError::Io)
}

pub fn read_event(reader: &mut impl BufRead) -> Result<Option<Event>, ProtocolError> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(ProtocolError::Io)?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(ProtocolError::UnterminatedFrame)
            };
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(count) > MAX_FRAME_BYTES {
            return Err(ProtocolError::OversizedFrame);
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            return decode_event(&bytes).map(Some);
        }
    }
}

pub type Event = EventWire;
