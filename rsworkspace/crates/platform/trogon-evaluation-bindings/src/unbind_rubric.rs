use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

use crate::error::{CommandInvalidError, EvaluationBindingDecideError, EvaluationBindingEvolveError};
use crate::state::{BindingStatus, EvaluationBindingState};

/// Ends a binding. Results already recorded stay; no new result may be recorded against it.
#[derive(Clone, Debug, PartialEq)]
pub struct UnbindRubric {
    binding_id: v1::EvaluationBindingId,
}

impl UnbindRubric {
    pub fn new(command: v1::UnbindRubric) -> Result<Self, CommandInvalidError> {
        let binding_id = command
            .binding_id
            .into_option()
            .filter(|id| !id.value.is_empty())
            .ok_or(CommandInvalidError { field: "binding_id" })?;
        Ok(Self { binding_id })
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id.value
    }
}

impl Decider for UnbindRubric {
    type StreamId = str;
    type State = EvaluationBindingState;
    type Event = v1::EvaluationBindingEvent;
    type DecideError = EvaluationBindingDecideError;
    type EvolveError = EvaluationBindingEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        &self.binding_id.value
    }

    fn initial_state() -> Self::State {
        crate::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        crate::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        if !matches!(state.status, BindingStatus::Bound(_)) {
            return Err(EvaluationBindingDecideError::NotFound);
        }

        Ok(Decision::event(v1::EvaluationBindingEvent {
            event: Some(
                v1::RubricUnbound {
                    binding_id: command.binding_id.clone().into(),
                }
                .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests;
