use trogon_decider::{Decider, Decision, WritePrecondition};
use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::example::{
    TurnOnCommand, LIGHT_STATE_SCHEMA_VERSION, TURN_ON_TYPE_URL, v1, state_v1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrongEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrongEventCommand {
    id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rejected")]
struct WrongEventDecideError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("evolve failed")]
struct WrongEventEvolveError;

impl Decider for WrongEventCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = WrongEvent;
    type DecideError = WrongEventDecideError;
    type EvolveError = WrongEventEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id.as_str()
    }

    fn initial_state() -> Self::State {
        state_v1::State {
            on: Some(false),
            turn_on_count: Some(0),
        }
    }

    fn evolve(_state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Err(WrongEventEvolveError)
    }

    fn decide(_state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Err(WrongEventDecideError)
    }
}

export_decider!(
    TurnOnCommand {
        type_url = TURN_ON_TYPE_URL,
        proto = v1::TurnOn,
        module = "example.light",
        version = "0.1.0",
        state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
    },
    WrongEventCommand {
        type_url = "type.googleapis.com/trogonai.example.light.v1.Wrong",
        proto = v1::TurnOn,
        module = "example.light",
        version = "0.1.0",
        state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
    },
);

fn main() {}
