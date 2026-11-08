use std::collections::BTreeSet;

use trogonai_proto::evaluation::bindings::v1alpha1::EvaluationBindingEvent;
use trogonai_proto::evaluation::bindings::{EvaluationBindingEventCase, v1alpha1::EvaluationBinding};

use crate::error::EvaluationBindingEvolveError;

/// Where the binding is in its life. A binding is immutable once bound, so there is no state in
/// which it was edited, only bound and then unbound.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BindingStatus {
    #[default]
    Missing,
    Bound(EvaluationBinding),
    Unbound,
}

/// Fold of one binding stream: its life and the ids of the results already recorded on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluationBindingState {
    pub status: BindingStatus,
    pub recorded_results: BTreeSet<String>,
}

pub fn initial_state() -> EvaluationBindingState {
    EvaluationBindingState::default()
}

pub fn evolve(
    mut state: EvaluationBindingState,
    event: &EvaluationBindingEvent,
) -> Result<EvaluationBindingState, EvaluationBindingEvolveError> {
    let event = event.event.as_ref().ok_or(EvaluationBindingEvolveError::MissingEvent)?;

    match event {
        EvaluationBindingEventCase::RubricBound(bound) => {
            if state.status != BindingStatus::Missing {
                return Err(EvaluationBindingEvolveError::BoundTwice);
            }
            let binding = bound
                .binding
                .as_option()
                .ok_or(EvaluationBindingEvolveError::MissingField { field: "binding" })?;
            state.status = BindingStatus::Bound(binding.clone());
        }
        EvaluationBindingEventCase::RubricUnbound(_) => {
            if !matches!(state.status, BindingStatus::Bound(_)) {
                return Err(EvaluationBindingEvolveError::UnboundWithoutActiveBinding);
            }
            state.status = BindingStatus::Unbound;
        }
        EvaluationBindingEventCase::EvaluationResultRecorded(recorded) => {
            if !matches!(state.status, BindingStatus::Bound(_)) {
                return Err(EvaluationBindingEvolveError::RecordedWithoutActiveBinding);
            }
            let id = recorded
                .evaluation_result_id
                .as_option()
                .ok_or(EvaluationBindingEvolveError::MissingField {
                    field: "evaluation_result_id",
                })?;
            if !state.recorded_results.insert(id.value.clone()) {
                return Err(EvaluationBindingEvolveError::ResultRecordedTwice(id.value.clone()));
            }
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests;
