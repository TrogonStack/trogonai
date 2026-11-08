//! Builders shared by the decider tests.

use trogonai_proto::agents::agents::v1alpha1::{
    AgentId, Digest, EditCaps, EditableSettingsPath, EditableSurface, PlatformComponent,
};
use trogonai_proto::evaluation::bindings::FindingTargetCase;
use trogonai_proto::evaluation::bindings::v1alpha1::__buffa::oneof::score::Value as ScoreValue;
use trogonai_proto::evaluation::bindings::v1alpha1::{
    self as v1, EvaluationBinding, EvaluationBindingEvent, EvaluationBindingId, EvaluationResultId, Finding,
    FindingTarget, JudgeInputCoverage, Score,
};
use trogonai_proto::evaluation::rubrics::v1alpha1::{RubricId, RubricVersionRef};
use trogonai_proto::hierarchy::v1alpha1::NodeId;
use trogonai_proto::policy::subjects::v1alpha1::__buffa::oneof::policy_subject::Subject;
use trogonai_proto::policy::subjects::v1alpha1::PolicySubject;
use trogonai_proto::session::sessions::v1alpha1::{SessionId, SessionOrdinal, SessionPositionRef};

use crate::facts::{FindingEvidence, JudgeScope, RubricVersionStanding, WithheldVocabularyScreen};
use crate::{BindRubric, RecordEvaluationResult, UnbindRubric};

pub const BINDING_ID: &str = "binding-1";
pub const JUDGE: &str = "agent-judge";
pub const SUBJECT: &str = "agent-subject";

pub fn agent_subject(id: &str) -> PolicySubject {
    PolicySubject {
        subject: Some(Subject::AgentId(Box::new(AgentId { value: id.to_string() }))),
    }
}

pub fn node_subject(id: &str) -> PolicySubject {
    PolicySubject {
        subject: Some(Subject::Node(Box::new(NodeId { value: id.to_string() }))),
    }
}

pub fn binding_with(id: &str, subject: PolicySubject, judge: &str) -> EvaluationBinding {
    EvaluationBinding {
        binding_id: EvaluationBindingId { value: id.to_string() }.into(),
        rubric_version_ref: RubricVersionRef {
            rubric_id: RubricId {
                value: "rubric-1".to_string(),
            }
            .into(),
            version: "v1".to_string(),
        }
        .into(),
        subject: subject.into(),
        judge: AgentId {
            value: judge.to_string(),
        }
        .into(),
    }
}

pub fn binding() -> EvaluationBinding {
    binding_with(BINDING_ID, agent_subject(SUBJECT), JUDGE)
}

pub fn bind_command_proto(binding: EvaluationBinding) -> v1::BindRubric {
    v1::BindRubric {
        binding: binding.into(),
    }
}

pub fn bind_command() -> BindRubric {
    BindRubric::new(
        bind_command_proto(binding()),
        RubricVersionStanding::Available,
        JudgeScope::OutsideSubject,
    )
    .unwrap()
}

pub fn unbind_command() -> UnbindRubric {
    UnbindRubric::new(v1::UnbindRubric {
        binding_id: EvaluationBindingId {
            value: BINDING_ID.to_string(),
        }
        .into(),
    })
    .unwrap()
}

pub fn bound() -> EvaluationBindingEvent {
    EvaluationBindingEvent {
        event: Some(
            v1::RubricBound {
                binding: binding().into(),
            }
            .into(),
        ),
    }
}

pub fn unbound() -> EvaluationBindingEvent {
    EvaluationBindingEvent {
        event: Some(
            v1::RubricUnbound {
                binding_id: EvaluationBindingId {
                    value: BINDING_ID.to_string(),
                }
                .into(),
            }
            .into(),
        ),
    }
}

pub fn settings_finding(path: &str) -> Finding {
    Finding {
        category: "instructions-too-vague".to_string(),
        target: FindingTarget {
            target: Some(FindingTargetCase::SettingsPath(path.to_string())),
        }
        .into(),
        confidence: 0.75,
        rationale_ref: Default::default(),
    }
}

pub fn component_finding(component: PlatformComponent) -> Finding {
    Finding {
        category: "missing-skill".to_string(),
        target: FindingTarget {
            target: Some(FindingTargetCase::PlatformComponent(component.into())),
        }
        .into(),
        confidence: 0.5,
        rationale_ref: Default::default(),
    }
}

pub fn surface(paths: &[&str], components: &[PlatformComponent]) -> EditableSurface {
    EditableSurface {
        settings_paths: paths
            .iter()
            .map(|path| EditableSettingsPath {
                path: (*path).to_string(),
                max_bytes: 1024,
            })
            .collect(),
        platform_components: components.iter().map(|component| (*component).into()).collect(),
        caps: EditCaps {
            max_staged_skills: 1,
            max_staged_skill_bytes: 1024,
            max_candidate_bytes: 4096,
        }
        .into(),
    }
}

pub fn evidence(surface: EditableSurface) -> FindingEvidence {
    FindingEvidence {
        editable_surface: surface,
        withheld_vocabulary: WithheldVocabularyScreen::Clear,
    }
}

pub fn session_position(session: &str, ordinal: u64) -> SessionPositionRef {
    SessionPositionRef {
        session_id: SessionId {
            value: session.to_string(),
        }
        .into(),
        through_ordinal: SessionOrdinal { value: ordinal }.into(),
    }
}

pub fn record_proto(result_id: &str, findings: Vec<Finding>) -> v1::RecordEvaluationResult {
    v1::RecordEvaluationResult {
        evaluation_result_id: EvaluationResultId {
            value: result_id.to_string(),
        }
        .into(),
        binding_id: EvaluationBindingId {
            value: BINDING_ID.to_string(),
        }
        .into(),
        evaluated_ref: session_position("session-evaluated", 12).into(),
        candidate_ref: Default::default(),
        score: Score {
            value: Some(ScoreValue::Scalar(0.5)),
        }
        .into(),
        rationale_ref: Default::default(),
        content_digest: Digest {
            algorithm: "sha256".to_string(),
            value: vec![7; 32],
        }
        .into(),
        findings,
        coverage: Default::default(),
        judge_session_ref: Default::default(),
    }
}

pub fn record_command(result_id: &str, findings: Vec<Finding>, evidence: FindingEvidence) -> RecordEvaluationResult {
    RecordEvaluationResult::new(record_proto(result_id, findings), evidence).unwrap()
}

/// The event `RecordEvaluationResult` is expected to emit for `command`.
pub fn recorded_from(command: &v1::RecordEvaluationResult) -> EvaluationBindingEvent {
    EvaluationBindingEvent {
        event: Some(
            v1::EvaluationResultRecorded {
                evaluation_result_id: command.evaluation_result_id.clone(),
                binding_id: command.binding_id.clone(),
                evaluated_ref: command.evaluated_ref.clone(),
                candidate_ref: command.candidate_ref.clone(),
                score: command.score.clone(),
                rationale_ref: command.rationale_ref.clone(),
                content_digest: command.content_digest.clone(),
                findings: command.findings.clone(),
                coverage: command.coverage.clone(),
                judge_session_ref: command.judge_session_ref.clone(),
            }
            .into(),
        ),
    }
}

pub fn coverage() -> JudgeInputCoverage {
    JudgeInputCoverage {
        effective_through: 11,
        withheld_count: 2,
        contract_clamped_count: 1,
    }
}
