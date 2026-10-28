//! The exported descriptor must preserve the policies native command execution uses.

use std::convert::Infallible;

use buffa::Message as _;
use trogon_decider::{
    Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, SnapshotCadence,
    WritePrecondition,
};
use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, REMOVE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL, v1,
};

use __trogon_decider_bindings::{Guest, SnapshotPolicy, WritePrecondition as GuestWritePrecondition};

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
    type State = v1::SchedulePaused;
    type Event = PausedEvent;
    type DecideError = Infallible;
    type EvolveError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = P::WRITE_PRECONDITION;
    const SNAPSHOT_CADENCE: SnapshotCadence = P::SNAPSHOT_CADENCE;

    fn stream_id(&self) -> &str {
        self.0.schedule_id()
    }

    fn initial_state() -> Self::State {
        v1::SchedulePaused::default()
    }

    fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(event.0.clone())
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
