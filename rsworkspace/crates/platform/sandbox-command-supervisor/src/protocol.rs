pub use crate::constants::*;
use crate::event_wire::validate_event;
pub use crate::{Cancellation, Certainty, Delivery, PlatformSupport, ReleaseState, TimeoutState, Truncation};
pub use crate::{
    Command, CommandWire, ErrorCode, Event, EventWire, ExecutionId, Identity, Phase, ProtocolError, Stream,
    SupervisorError,
};
use serde::Serialize;
use std::io::{BufRead, Write};

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
    let wire: EventWire = serde_json::from_slice(bytes).map_err(ProtocolError::InvalidJson)?;
    Event::try_from(wire)
}

fn encode_frame(value: &impl Serialize) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = serde_json::to_vec(value).map_err(ProtocolError::InvalidJson)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn encode_event(event: &(impl AsRef<EventWire> + ?Sized)) -> Result<Vec<u8>, ProtocolError> {
    let wire = event.as_ref();
    validate_event(wire)?;
    encode_frame(wire)
}

pub fn write_command(writer: &mut impl Write, command: &CommandWire) -> Result<(), ProtocolError> {
    Command::try_from(command.clone())?;
    writer.write_all(&encode_frame(command)?).map_err(ProtocolError::Io)?;
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
