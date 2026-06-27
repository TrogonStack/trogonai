//! M0 spike: hand-written Light decider guest (no macro).

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
mod bindings {
    wit_bindgen::generate!({
        world: "decider",
        path: "../trogon-decider-wit/wit",
        generate_all,
    });
}

use std::cell::RefCell;

use buffa::Message as _;
use trogon_decider::{
    Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, WritePrecondition,
};
use trogonai_proto::example::{LIGHT_STATE_SCHEMA_VERSION, LightEventCase, TURN_ON_TYPE_URL, state_v1, v1};

use bindings::exports::trogon::decider::handler::{
    AnyEnvelope, CommandEnvelope, CommandSpec, DecideError, DomainError, Guest, GuestSession, ModuleDescriptor,
    WritePrecondition as WitWritePrecondition,
};

struct Component;

struct Session {
    state: RefCell<state_v1::State>,
}

impl Guest for Component {
    fn descriptor() -> ModuleDescriptor {
        ModuleDescriptor {
            name: "example.light".to_string(),
            version: "0.1.0".to_string(),
            commands: vec![CommandSpec {
                command_type: TURN_ON_TYPE_URL.to_string(),
                write_precondition: Some(WitWritePrecondition::NoStream),
            }],
        }
    }

    fn stream_id(command: CommandEnvelope) -> Result<String, DomainError> {
        let cmd = decode_turn_on(command)?;
        Ok(cmd.light_id)
    }

    type Session = Session;
}

impl GuestSession for Session {
    fn new(snapshot: Option<Vec<u8>>) -> Self {
        let state = match snapshot {
            Some(bytes) => decode_snapshot(&bytes).unwrap_or_else(|_| initial_state()),
            None => initial_state(),
        };
        Self {
            state: RefCell::new(state),
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
    )]
    fn evolve(&self, events: Vec<AnyEnvelope>) -> Result<(), DomainError> {
        let mut state = self.state.borrow().clone();
        for envelope in events {
            let decoded =
                <v1::LightEvent as EventDecode>::decode(EventData::new(envelope.type_.as_str(), &envelope.payload))
                    .map_err(decode_fault)?;

            let event = match decoded {
                EventDecodeOutcome::Decoded(event) => event,
                EventDecodeOutcome::Skipped => {
                    return Err(DomainError {
                        code: "unknown-event".to_string(),
                        message: format!("unsupported event type '{}'", envelope.type_),
                    });
                }
            };

            state = TurnOnCmd::evolve(state, &event).map_err(evolve_fault)?;
        }
        *self.state.borrow_mut() = state;
        Ok(())
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "decider guest bridge dispatch path; the disallowed_methods rule targets test code calling decide/evolve directly"
    )]
    fn decide(&self, command: CommandEnvelope) -> Result<Vec<AnyEnvelope>, DecideError> {
        let cmd = decode_turn_on(command).map_err(DecideError::Faulted)?;
        let state = self.state.borrow();
        let decision = TurnOnCmd::decide(&state, &cmd).map_err(decide_rejected)?;

        let events = match decision {
            Decision::Events(batch) => batch.into_vec(),
            Decision::Act(_) => {
                return Err(DecideError::Faulted(DomainError {
                    code: "unsupported-decision".to_string(),
                    message: "Act decisions are not supported in WASM v1".to_string(),
                }));
            }
            _ => {
                return Err(DecideError::Faulted(DomainError {
                    code: "unsupported-decision".to_string(),
                    message: "unsupported decision variant".to_string(),
                }));
            }
        };

        events
            .into_iter()
            .map(encode_event_envelope)
            .collect::<Result<Vec<_>, _>>()
            .map_err(DecideError::Faulted)
    }

    fn snapshot(&self) -> Option<Vec<u8>> {
        Some(encode_snapshot(&self.state.borrow()))
    }
}

#[derive(Debug, Clone)]
struct TurnOnCmd {
    light_id: String,
}

impl Decider for TurnOnCmd {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::LightEvent;
    type DecideError = TurnOnDecideError;
    type EvolveError = TurnOnEvolveError;

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        self.light_id.as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        let Some(LightEventCase::LightTurnedOn(turned_on)) = event.event.as_ref() else {
            return Err(TurnOnEvolveError);
        };

        Ok(state_v1::State {
            on: Some(true),
            turn_on_count: Some(turned_on.turn_on_count),
        })
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        if state.on == Some(true) {
            return Err(TurnOnDecideError::AlreadyOn {
                light_id: command.light_id.clone(),
            });
        }

        let next_count = state.turn_on_count.unwrap_or(0).saturating_add(1);

        Ok(Decision::event(v1::LightEvent {
            event: Some(LightEventCase::from(v1::LightTurnedOn {
                light_id: command.light_id.clone(),
                turn_on_count: next_count,
            })),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum TurnOnDecideError {
    #[error("light '{light_id}' is already on")]
    AlreadyOn { light_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("light event is missing its event case")]
struct TurnOnEvolveError;

fn initial_state() -> state_v1::State {
    state_v1::State {
        on: Some(false),
        turn_on_count: Some(0),
    }
}

fn decode_turn_on(envelope: CommandEnvelope) -> Result<TurnOnCmd, DomainError> {
    if envelope.type_ != TURN_ON_TYPE_URL {
        return Err(DomainError {
            code: "invalid-command".to_string(),
            message: format!("unknown command type '{}'", envelope.type_),
        });
    }

    let proto = v1::TurnOn::decode_from_slice(&envelope.payload).map_err(|source| DomainError {
        code: "invalid-command".to_string(),
        message: source.to_string(),
    })?;

    if proto.light_id.is_empty() {
        return Err(DomainError {
            code: "invalid-command".to_string(),
            message: "turn-on command is missing light_id".to_string(),
        });
    }

    Ok(TurnOnCmd {
        light_id: proto.light_id,
    })
}

fn encode_event_envelope(event: v1::LightEvent) -> Result<AnyEnvelope, DomainError> {
    let payload = EventEncode::encode(&event).map_err(|source| DomainError {
        code: "encode-failed".to_string(),
        message: source.to_string(),
    })?;
    Ok(AnyEnvelope {
        type_: EventType::event_type(&event)
            .map_err(|source| DomainError {
                code: "encode-failed".to_string(),
                message: source.to_string(),
            })?
            .to_string(),
        payload,
    })
}

fn encode_snapshot(state: &state_v1::State) -> Vec<u8> {
    let payload = state.encode_to_vec();
    encode_snapshot_frame(LIGHT_STATE_SCHEMA_VERSION, payload)
}

fn decode_snapshot(bytes: &[u8]) -> Result<state_v1::State, DomainError> {
    let (schema_version, payload) = decode_snapshot_frame(bytes)?;
    if schema_version != LIGHT_STATE_SCHEMA_VERSION {
        return Err(DomainError {
            code: "snapshot-version-mismatch".to_string(),
            message: format!("expected schema '{LIGHT_STATE_SCHEMA_VERSION}', got '{schema_version}'"),
        });
    }
    state_v1::State::decode_from_slice(payload).map_err(|source| DomainError {
        code: "snapshot-decode-failed".to_string(),
        message: source.to_string(),
    })
}

fn encode_snapshot_frame(schema_version: &str, payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, schema_version);
    write_bytes_field(&mut out, 2, &payload);
    out
}

fn decode_snapshot_frame(bytes: &[u8]) -> Result<(String, &[u8]), DomainError> {
    let mut schema_version = None;
    let mut payload = None;
    let mut offset = 0;

    while offset < bytes.len() {
        let (tag, wire_type, next) = read_key(bytes, offset)?;
        offset = next;
        match tag {
            1 => {
                let (value, next) = read_length_delimited(bytes, offset)?;
                schema_version = Some(String::from_utf8(value.to_vec()).map_err(|_| snapshot_fault("invalid utf8"))?);
                offset = next;
            }
            2 => {
                let (value, next) = read_length_delimited(bytes, offset)?;
                payload = Some(value);
                offset = next;
            }
            _ => {
                offset = skip_field(bytes, offset, wire_type)?;
            }
        }
    }

    Ok((
        schema_version.ok_or_else(|| snapshot_fault("missing schema_version"))?,
        payload.ok_or_else(|| snapshot_fault("missing payload"))?,
    ))
}

fn write_string_field(out: &mut Vec<u8>, field_number: u32, value: &str) {
    write_key(out, field_number, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn write_bytes_field(out: &mut Vec<u8>, field_number: u32, value: &[u8]) {
    write_key(out, field_number, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_key(out: &mut Vec<u8>, field_number: u32, wire_type: u32) {
    write_varint(out, ((field_number << 3) | wire_type) as u64);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint(bytes: &[u8], mut offset: usize) -> Result<(u64, usize), DomainError> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(offset).ok_or_else(|| snapshot_fault("unexpected eof"))?;
        offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(snapshot_fault("varint overflow"));
        }
    }
}

fn read_key(bytes: &[u8], offset: usize) -> Result<(u32, u32, usize), DomainError> {
    let (key, offset) = read_varint(bytes, offset)?;
    Ok(((key >> 3) as u32, (key & 0x07) as u32, offset))
}

fn read_length_delimited(bytes: &[u8], offset: usize) -> Result<(&[u8], usize), DomainError> {
    let (len, offset) = read_varint(bytes, offset)?;
    let len = len as usize;
    let end = offset
        .checked_add(len)
        .ok_or_else(|| snapshot_fault("length overflow"))?;
    let slice = bytes.get(offset..end).ok_or_else(|| snapshot_fault("unexpected eof"))?;
    Ok((slice, end))
}

fn skip_field(bytes: &[u8], offset: usize, wire_type: u32) -> Result<usize, DomainError> {
    match wire_type {
        0 => {
            let (_, offset) = read_varint(bytes, offset)?;
            Ok(offset)
        }
        1 => Ok(offset + 8),
        2 => {
            let (_, offset) = read_length_delimited(bytes, offset)?;
            Ok(offset)
        }
        5 => Ok(offset + 4),
        _ => Err(snapshot_fault("unsupported wire type")),
    }
}

fn snapshot_fault(message: &str) -> DomainError {
    DomainError {
        code: "snapshot-decode-failed".to_string(),
        message: message.to_string(),
    }
}

fn decode_fault<E: std::error::Error>(source: E) -> DomainError {
    DomainError {
        code: "decode-failed".to_string(),
        message: source.to_string(),
    }
}

fn evolve_fault(error: TurnOnEvolveError) -> DomainError {
    DomainError {
        code: "evolve-failed".to_string(),
        message: error.to_string(),
    }
}

fn decide_rejected(error: TurnOnDecideError) -> DecideError {
    DecideError::Rejected(DomainError {
        code: match &error {
            TurnOnDecideError::AlreadyOn { .. } => "already-on",
        }
        .to_string(),
        message: error.to_string(),
    })
}

bindings::export!(Component with_types_in bindings);
