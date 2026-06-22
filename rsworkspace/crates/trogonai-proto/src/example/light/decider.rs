use trogon_decider::{Decider, Decision, WritePrecondition};

use super::codec::{TurnOnCommand, TurnOnDecideError, TurnOnEvolveError};
use super::{LightEventCase, state_v1, v1};

impl Decider for TurnOnCommand {
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
        state_v1::State {
            on: Some(false),
            turn_on_count: Some(0),
        }
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
