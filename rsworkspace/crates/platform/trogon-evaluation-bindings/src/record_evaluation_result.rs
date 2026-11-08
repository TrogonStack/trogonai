use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

use crate::error::{CommandInvalidError, EvaluationBindingDecideError, EvaluationBindingEvolveError};
use crate::facts::{FindingEvidence, WithheldVocabularyScreen};
use crate::state::{BindingStatus, EvaluationBindingState};
use crate::surface::finding_inside_surface;

/// The most findings one result may carry (`record_evaluation_result.proto`).
const MAX_FINDINGS: usize = 64;

/// Records the score of one evaluated run, with its findings and the judge's own session.
///
/// `evidence` is the proven input the findings are judged against; see [`FindingEvidence`].
#[derive(Clone, Debug, PartialEq)]
pub struct RecordEvaluationResult {
    binding_id: String,
    result: v1::RecordEvaluationResult,
    evidence: FindingEvidence,
}

impl RecordEvaluationResult {
    pub fn new(command: v1::RecordEvaluationResult, evidence: FindingEvidence) -> Result<Self, CommandInvalidError> {
        let invalid = |field| CommandInvalidError { field };
        if command
            .evaluation_result_id
            .as_option()
            .is_none_or(|id| id.value.is_empty())
        {
            return Err(invalid("evaluation_result_id"));
        }
        let binding_id = command
            .binding_id
            .as_option()
            .map(|id| id.value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("binding_id"))?
            .to_string();
        if command.evaluated_ref.as_option().is_none() {
            return Err(invalid("evaluated_ref"));
        }
        if command.score.as_option().is_none_or(|score| score.value.is_none()) {
            return Err(invalid("score"));
        }
        if command.content_digest.as_option().is_none() {
            return Err(invalid("content_digest"));
        }
        if command.findings.len() > MAX_FINDINGS {
            return Err(invalid("findings"));
        }
        if command
            .findings
            .iter()
            .any(|finding| finding.target.as_option().is_none_or(|target| target.target.is_none()))
        {
            return Err(invalid("findings.target"));
        }

        Ok(Self {
            binding_id,
            result: command,
            evidence,
        })
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }

    fn result_id(&self) -> &str {
        self.result
            .evaluation_result_id
            .as_option()
            .map_or("", |id| id.value.as_str())
    }
}

impl Decider for RecordEvaluationResult {
    type StreamId = str;
    type State = EvaluationBindingState;
    type Event = v1::EvaluationBindingEvent;
    type DecideError = EvaluationBindingDecideError;
    type EvolveError = EvaluationBindingEvolveError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

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
        if !matches!(state.status, BindingStatus::Bound(_)) {
            return Err(EvaluationBindingDecideError::NotFound);
        }
        if state.recorded_results.contains(command.result_id()) {
            return Err(EvaluationBindingDecideError::ResultAlreadyRecorded);
        }
        // Surface first, then vocabulary: neither refusal says which finding or term tripped it.
        if command
            .result
            .findings
            .iter()
            .any(|finding| !finding_inside_surface(finding, &command.evidence.editable_surface))
        {
            return Err(EvaluationBindingDecideError::FindingOutsideEditableSurface);
        }
        if !command.result.findings.is_empty()
            && command.evidence.withheld_vocabulary == WithheldVocabularyScreen::Matched
        {
            return Err(EvaluationBindingDecideError::FindingMatchesWithheldVocabulary);
        }

        let result = &command.result;
        Ok(Decision::event(v1::EvaluationBindingEvent {
            event: Some(
                v1::EvaluationResultRecorded {
                    evaluation_result_id: result.evaluation_result_id.clone(),
                    binding_id: result.binding_id.clone(),
                    evaluated_ref: result.evaluated_ref.clone(),
                    candidate_ref: result.candidate_ref.clone(),
                    score: result.score.clone(),
                    rationale_ref: result.rationale_ref.clone(),
                    content_digest: result.content_digest.clone(),
                    findings: result.findings.clone(),
                    coverage: result.coverage.clone(),
                    judge_session_ref: result.judge_session_ref.clone(),
                }
                .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests;
