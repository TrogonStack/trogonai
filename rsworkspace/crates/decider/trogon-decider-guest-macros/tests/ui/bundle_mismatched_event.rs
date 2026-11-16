use std::convert::Infallible;
use trogon_decider::{
    Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, Evolve, WritePrecondition,
};
use trogon_decider_guest_sdk::export_decider;
use trogon_scheduler_domain::{CreateSchedule, ScheduleState};
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, SCHEDULES_STATE_SCHEMA_VERSION, v1};

struct WrongEvent;

impl Evolve<WrongEvent> for ScheduleState {
    type Error = Infallible;

    fn evolve(self, _event: &WrongEvent) -> Result<Self, Self::Error> {
        Ok(self)
    }
}

impl EventEncode for WrongEvent {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }
}

impl EventType for WrongEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("fixture.wrong-event")
    }
}

impl EventDecode for WrongEvent {
    type Error = Infallible;

    fn decode(_event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        Ok(EventDecodeOutcome::Skipped)
    }
}

struct WrongEventCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rejected")]
struct WrongEventDecideError;

impl TryFrom<v1::CreateSchedule> for WrongEventCommand {
    type Error = WrongEventDecideError;

    fn try_from(_value: v1::CreateSchedule) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl Decider for WrongEventCommand {
    type StreamId = str;
    type State = ScheduleState;
    type Event = WrongEvent;
    type DecideError = WrongEventDecideError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "wrong"
    }

    fn decide(_state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Err(WrongEventDecideError)
    }
}

export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
    WrongEventCommand {
        type_url = "type.googleapis.com/trogonai.scheduler.schedules.v1.Wrong",
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
);

fn main() {}
