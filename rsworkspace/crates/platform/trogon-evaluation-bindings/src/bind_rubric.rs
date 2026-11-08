use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::evaluation::bindings::v1alpha1::{self as v1, EvaluationBinding};
use trogonai_proto::policy::subjects::v1alpha1::__buffa::oneof::policy_subject::Subject;

use crate::error::{CommandInvalidError, EvaluationBindingDecideError, EvaluationBindingEvolveError};
use crate::facts::{JudgeScope, RubricVersionStanding};
use crate::state::{BindingStatus, EvaluationBindingState};

/// Binds a rubric version to a subject and a judge.
///
/// `rubric_version` and `judge_scope` are proven inputs: the rubric stream and the hierarchy are
/// read by the dispatcher, never by this decider.
#[derive(Clone, Debug, PartialEq)]
pub struct BindRubric {
    binding_id: String,
    binding: EvaluationBinding,
    rubric_version: RubricVersionStanding,
    judge_scope: JudgeScope,
}

impl BindRubric {
    pub fn new(
        command: v1::BindRubric,
        rubric_version: RubricVersionStanding,
        judge_scope: JudgeScope,
    ) -> Result<Self, CommandInvalidError> {
        let binding = command
            .binding
            .into_option()
            .ok_or(CommandInvalidError { field: "binding" })?;
        let binding_id = binding
            .binding_id
            .as_option()
            .map(|id| id.value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or(CommandInvalidError {
                field: "binding.binding_id",
            })?
            .to_string();
        let rubric_ref = binding.rubric_version_ref.as_option().ok_or(CommandInvalidError {
            field: "binding.rubric_version_ref",
        })?;
        if rubric_ref.version.is_empty() || rubric_ref.rubric_id.as_option().is_none_or(|id| id.value.is_empty()) {
            return Err(CommandInvalidError {
                field: "binding.rubric_version_ref",
            });
        }
        match binding.subject.as_option().and_then(|subject| subject.subject.as_ref()) {
            Some(Subject::AgentId(id)) if !id.value.is_empty() => {}
            Some(Subject::Node(id)) if !id.value.is_empty() => {}
            _ => {
                return Err(CommandInvalidError {
                    field: "binding.subject",
                });
            }
        }
        if binding.judge.as_option().is_none_or(|judge| judge.value.is_empty()) {
            return Err(CommandInvalidError { field: "binding.judge" });
        }

        Ok(Self {
            binding_id,
            binding,
            rubric_version,
            judge_scope,
        })
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }

    fn judge_is_subject(&self) -> bool {
        let judge = self.binding.judge.as_option().map(|judge| judge.value.as_str());
        let subject = self
            .binding
            .subject
            .as_option()
            .and_then(|subject| subject.subject.as_ref());
        match (subject, judge) {
            (Some(Subject::AgentId(subject)), Some(judge)) => subject.value == judge,
            _ => false,
        }
    }
}

impl Decider for BindRubric {
    type StreamId = str;
    type State = EvaluationBindingState;
    type Event = v1::EvaluationBindingEvent;
    type DecideError = EvaluationBindingDecideError;
    type EvolveError = EvaluationBindingEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;

    fn stream_id(&self) -> &Self::StreamId {
        &self.binding_id
    }

    fn initial_state() -> Self::State {
        crate::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        crate::state::evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        // A bound or unbound stream both refuse: rebinding is a new binding id.
        if state.status != BindingStatus::Missing {
            return Err(EvaluationBindingDecideError::AlreadyExists);
        }
        if command.rubric_version != RubricVersionStanding::Available {
            return Err(EvaluationBindingDecideError::RubricVersionNotFound);
        }
        if command.judge_is_subject() || command.judge_scope == JudgeScope::InsideSubject {
            return Err(EvaluationBindingDecideError::JudgeMatchesSubject);
        }

        Ok(Decision::event(v1::EvaluationBindingEvent {
            event: Some(
                v1::RubricBound {
                    binding: command.binding.clone().into(),
                }
                .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests;
