//! The exported descriptor must preserve the policies native command execution uses.

use std::convert::Infallible;

use buffa::Message as _;
use trogon_decider::{
    Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, Evolve, SnapshotCadence,
    StateSnapshot, WritePrecondition,
};
use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, REMOVE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL, v1,
};

use __trogon_decider_bindings::{Guest, GuestSession, SnapshotPolicy, WritePrecondition as GuestWritePrecondition};

trait Declaration {
    const WRITE_PRECONDITION: WritePrecondition;
    const SNAPSHOT_CADENCE: SnapshotCadence;

    fn schedule_id(&self) -> &str;
}

macro_rules! declaration {
    ($proto:ty, $precondition:expr, $cadence:expr) => {
        impl Declaration for $proto {
            const WRITE_PRECONDITION: WritePrecondition = $precondition;
            const SNAPSHOT_CADENCE: SnapshotCadence = $cadence;

            fn schedule_id(&self) -> &str {
                &self.schedule_id
            }
        }
    };
}

declaration!(v1::CreateSchedule, WritePrecondition::NoStream, SnapshotCadence::Never);
declaration!(
    v1::PauseSchedule,
    WritePrecondition::StreamUnchanged,
    SnapshotCadence::every_events(17)
);
declaration!(
    v1::ResumeSchedule,
    WritePrecondition::StreamExists,
    SnapshotCadence::every_events(1)
);
declaration!(
    v1::RemoveSchedule,
    WritePrecondition::Any,
    SnapshotCadence::every_events(u64::MAX)
);

struct DeclaredCommand<P>(P);

impl<P> From<P> for DeclaredCommand<P> {
    fn from(value: P) -> Self {
        Self(value)
    }
}

struct PausedEvent(v1::SchedulePaused);

#[derive(Clone, Debug, PartialEq)]
struct DeclaredState(v1::SchedulePaused);

impl Default for DeclaredState {
    fn default() -> Self {
        Self(v1::SchedulePaused {
            schedule_id: "initial".to_string(),
        })
    }
}

impl Evolve<PausedEvent> for DeclaredState {
    type Error = Infallible;

    fn evolve(self, event: &PausedEvent) -> Result<Self, Self::Error> {
        Ok(Self(event.0.clone()))
    }
}

impl StateSnapshot for DeclaredState {
    type Payload = v1::SchedulePaused;
}

impl From<v1::SchedulePaused> for DeclaredState {
    fn from(payload: v1::SchedulePaused) -> Self {
        Self(payload)
    }
}

impl AsRef<v1::SchedulePaused> for DeclaredState {
    fn as_ref(&self) -> &v1::SchedulePaused {
        &self.0
    }
}

impl EventEncode for PausedEvent {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.0.encode_to_vec())
    }
}

impl EventType for PausedEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("trogonai.scheduler.schedules.v1.SchedulePaused")
    }
}

impl EventDecode for PausedEvent {
    type Error = buffa::DecodeError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        if event.event_type != "trogonai.scheduler.schedules.v1.SchedulePaused" {
            return Ok(EventDecodeOutcome::Skipped);
        }
        v1::SchedulePaused::decode_from_slice(event.payload).map(|event| EventDecodeOutcome::Decoded(Self(event)))
    }
}

impl<P: Declaration> Decider for DeclaredCommand<P> {
    type StreamId = str;
    type State = DeclaredState;
    type Event = PausedEvent;
    type DecideError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = P::WRITE_PRECONDITION;
    const SNAPSHOT_CADENCE: SnapshotCadence = P::SNAPSHOT_CADENCE;

    fn stream_id(&self) -> &str {
        self.0.schedule_id()
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(PausedEvent(v1::SchedulePaused {
            schedule_id: command.0.schedule_id().to_string(),
        })))
    }
}

export_decider!(
    DeclaredCommand<v1::CreateSchedule> {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "fixture.policies",
        version = "0.1.0",
        state_schema_version = "fixture-state/v1",
    },
    DeclaredCommand<v1::PauseSchedule> {
        type_url = PAUSE_SCHEDULE_TYPE_URL,
        proto = v1::PauseSchedule,
        module = "fixture.policies",
        version = "0.1.0",
        state_schema_version = "fixture-state/v1",
    },
    DeclaredCommand<v1::ResumeSchedule> {
        type_url = RESUME_SCHEDULE_TYPE_URL,
        proto = v1::ResumeSchedule,
        module = "fixture.policies",
        version = "0.1.0",
        state_schema_version = "fixture-state/v1",
    },
    DeclaredCommand<v1::RemoveSchedule> {
        type_url = REMOVE_SCHEDULE_TYPE_URL,
        proto = v1::RemoveSchedule,
        module = "fixture.policies",
        version = "0.1.0",
        state_schema_version = "fixture-state/v1",
    },
);

#[test]
fn exported_session_restores_and_encodes_state_payload() {
    let initial = Session::new(None);
    let snapshot = trogon_decider_guest_sdk::encode_current_state(&DeclaredState::default(), "fixture-state/v1");
    assert_eq!(initial.snapshot(), Some(snapshot.clone()));
    let restored = Session::new(Some(snapshot.clone()));
    assert_eq!(restored.snapshot(), Some(snapshot));

    let empty_payload_frame = b"\x0a\x10fixture-state/v1\x12\x00".to_vec();
    let empty_state_snapshot =
        trogon_decider_guest_sdk::encode_current(&v1::SchedulePaused::default(), "fixture-state/v1");
    assert_ne!(initial.snapshot(), Some(empty_state_snapshot.clone()));
    assert_eq!(
        Session::new(Some(empty_payload_frame)).snapshot(),
        Some(empty_state_snapshot)
    );

    let state = DeclaredState(v1::SchedulePaused {
        schedule_id: "restored".to_string(),
    });
    let snapshot = trogon_decider_guest_sdk::encode_current_state(&state, "fixture-state/v1");
    assert_eq!(Session::new(Some(snapshot.clone())).snapshot(), Some(snapshot));
}

#[test]
fn exported_descriptor_preserves_each_commands_concurrency_and_snapshot_policy() {
    let descriptor = Component::descriptor();
    let expected = [
        (
            CREATE_SCHEDULE_TYPE_URL,
            <DeclaredCommand<v1::CreateSchedule> as Decider>::SNAPSHOT_CADENCE,
        ),
        (
            PAUSE_SCHEDULE_TYPE_URL,
            <DeclaredCommand<v1::PauseSchedule> as Decider>::SNAPSHOT_CADENCE,
        ),
        (
            RESUME_SCHEDULE_TYPE_URL,
            <DeclaredCommand<v1::ResumeSchedule> as Decider>::SNAPSHOT_CADENCE,
        ),
        (
            REMOVE_SCHEDULE_TYPE_URL,
            <DeclaredCommand<v1::RemoveSchedule> as Decider>::SNAPSHOT_CADENCE,
        ),
    ];
    assert_eq!(descriptor.commands.len(), expected.len());
    assert!(matches!(
        descriptor.commands[0].write_precondition,
        GuestWritePrecondition::NoStream
    ));
    assert!(matches!(
        descriptor.commands[1].write_precondition,
        GuestWritePrecondition::StreamUnchanged
    ));
    assert!(matches!(
        descriptor.commands[2].write_precondition,
        GuestWritePrecondition::StreamExists
    ));
    assert!(matches!(
        descriptor.commands[3].write_precondition,
        GuestWritePrecondition::Any
    ));
    for (command, (type_url, cadence)) in descriptor.commands.iter().zip(expected) {
        assert_eq!(command.command_type, type_url);
        let frequency = match command.snapshot_policy {
            SnapshotPolicy::NoSnapshot => None,
            SnapshotPolicy::Frequency(frequency) => Some(frequency),
        };
        assert_eq!(frequency, cadence.frequency().map(std::num::NonZeroU64::get));
    }
}
