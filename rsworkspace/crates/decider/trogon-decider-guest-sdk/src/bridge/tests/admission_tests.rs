use buffa::Message as _;
use trogonai_proto::scheduler::schedules::v1::PauseSchedule;

use super::*;

const COMMAND_TYPE: &str = "type.googleapis.com/trogonai.scheduler.schedules.v1.PauseSchedule";

impl CommandEnvelopeView for WireEvent {
    fn command_type(&self) -> &str {
        &self.type_url
    }

    fn command_payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FixturePause {
    schedule_id: String,
}

#[derive(Debug, thiserror::Error)]
#[error("schedule id is empty")]
struct EmptyScheduleIdError;

impl TryFrom<PauseSchedule> for FixturePause {
    type Error = EmptyScheduleIdError;

    fn try_from(value: PauseSchedule) -> Result<Self, Self::Error> {
        if value.schedule_id.is_empty() {
            return Err(EmptyScheduleIdError);
        }
        Ok(Self {
            schedule_id: value.schedule_id,
        })
    }
}

fn decode(envelope: WireEvent) -> Result<FixturePause, WireError> {
    decode_command::<_, PauseSchedule, _, _>(COMMAND_TYPE, envelope)
}

#[test]
fn wrong_command_type_is_refused_before_payload_decoding() {
    let error = decode(WireEvent {
        type_url: "type.googleapis.com/other.Command".to_string(),
        payload: vec![0x0a, 0x02, b'x'],
    })
    .unwrap_err();

    assert_eq!(error.kind, WireErrorKind::Faulted);
    assert_eq!(error.code, "invalid-command");
    assert_eq!(
        error.message,
        "unknown command type 'type.googleapis.com/other.Command'"
    );
    assert!(error.details.is_empty());
}

#[test]
fn malformed_command_preserves_its_protobuf_decode_cause() {
    let payload = vec![0x0a, 0x02, b'x'];
    let cause = PauseSchedule::decode_from_slice(&payload).unwrap_err();
    let error = decode(WireEvent {
        type_url: COMMAND_TYPE.to_string(),
        payload,
    })
    .unwrap_err();

    assert_eq!(error.code, "invalid-command");
    assert_eq!(error.message, format!("failed to decode command payload: {cause}"));
    assert_eq!(error.details, vec![("cause.0".to_string(), cause.to_string())]);
}

#[test]
fn decoded_command_must_pass_its_domain_conversion() {
    let command = PauseSchedule {
        schedule_id: String::new(),
    };
    let error = decode(WireEvent {
        type_url: COMMAND_TYPE.to_string(),
        payload: command.encode_to_vec(),
    })
    .unwrap_err();

    assert_eq!(error.code, "invalid-command");
    assert_eq!(error.message, "failed to convert command: schedule id is empty");
    assert_eq!(
        error.details,
        vec![("cause.0".to_string(), EmptyScheduleIdError.to_string())]
    );
}

#[test]
fn admitted_command_preserves_its_identifier() {
    let command = PauseSchedule {
        schedule_id: "nightly-backup".to_string(),
    };
    let decoded = decode(WireEvent {
        type_url: COMMAND_TYPE.to_string(),
        payload: command.encode_to_vec(),
    })
    .unwrap();

    assert_eq!(decoded.schedule_id, command.schedule_id);
}

#[test]
fn unrecognized_history_does_not_change_existing_state() {
    let state = FixtureState::Funded { amount: 42 };
    let replayed = evolve_one::<OpenAndFund, WireError, _>(
        state,
        WireEvent {
            type_url: "future.event".to_string(),
            payload: vec![0xff],
        },
    )
    .unwrap();

    assert_eq!(replayed, state);
}

#[test]
fn malformed_known_history_is_a_decode_failure() {
    let error = evolve_one::<OpenAndFund, WireError, _>(
        FixtureState::Opened,
        WireEvent {
            type_url: "fixture.funded".to_string(),
            payload: vec![1, 2, 3],
        },
    )
    .unwrap_err();

    assert_eq!(error.code, "decode-failed");
    assert_eq!(error.message, "failed to decode event: fixture codec error");
    assert_eq!(
        error.details,
        vec![("cause.0".to_string(), FixtureCodecError.to_string())]
    );
}

#[test]
fn valid_history_in_an_invalid_state_is_an_evolve_failure() {
    let error = evolve_one::<OpenAndFund, WireError, _>(
        FixtureState::New,
        WireEvent {
            type_url: "fixture.funded".to_string(),
            payload: 42_u32.to_le_bytes().to_vec(),
        },
    )
    .unwrap_err();

    assert_eq!(error.code, "evolve-failed");
    assert_eq!(
        error.message,
        "failed to evolve state: invalid fixture event for current state"
    );
    assert_eq!(
        error.details,
        vec![("cause.0".to_string(), FixtureEvolveError.to_string())]
    );
}
