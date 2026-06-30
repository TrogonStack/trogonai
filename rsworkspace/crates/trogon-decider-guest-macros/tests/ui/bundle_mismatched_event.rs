use trogon_decider::{Decider, Decision};
use trogon_decider_guest_sdk::export_decider;
use trogon_scheduler_domain::CreateSchedule;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, SCHEDULES_STATE_SCHEMA_VERSION, state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrongEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrongEventCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rejected")]
struct WrongEventDecideError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("evolve failed")]
struct WrongEventEvolveError;

impl TryFrom<v1::CreateSchedule> for WrongEventCommand {
    type Error = WrongEventDecideError;

    fn try_from(_value: v1::CreateSchedule) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

impl Decider for WrongEventCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = WrongEvent;
    type DecideError = WrongEventDecideError;
    type EvolveError = WrongEventEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        "wrong"
    }

    fn initial_state() -> Self::State {
        state_v1::State::default()
    }

    fn evolve(_state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Err(WrongEventEvolveError)
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
