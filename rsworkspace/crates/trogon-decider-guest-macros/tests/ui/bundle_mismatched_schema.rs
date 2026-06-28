use trogon_decider::{Decider, Decision};
use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::example::{
    LIGHT_STATE_SCHEMA_VERSION, TURN_ON_TYPE_URL, TurnOnCommand, state_v1, v1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecondCommand {
    id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rejected")]
struct SecondDecideError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("evolve failed")]
struct SecondEvolveError;

impl TryFrom<v1::TurnOn> for SecondCommand {
    type Error = SecondDecideError;

    fn try_from(value: v1::TurnOn) -> Result<Self, Self::Error> {
        Ok(Self { id: value.light_id })
    }
}

impl Decider for SecondCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::LightEvent;
    type DecideError = SecondDecideError;
    type EvolveError = SecondEvolveError;

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
        Err(SecondEvolveError)
    }

    fn decide(_state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Err(SecondDecideError)
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
    SecondCommand {
        type_url = "type.googleapis.com/trogonai.example.light.v1.Second",
        proto = v1::TurnOn,
        module = "example.light",
        version = "0.1.0",
        state_schema_version = "type.googleapis.com/trogonai.example.light.state.v2.State",
    },
);

fn main() {}
