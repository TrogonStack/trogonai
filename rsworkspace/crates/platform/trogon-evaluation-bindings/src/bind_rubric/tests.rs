use trogon_decider::testing::TestCase;
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

use super::*;
use crate::fixtures::*;

fn command(
    binding: v1::EvaluationBinding,
    rubric_version: RubricVersionStanding,
    judge_scope: JudgeScope,
) -> BindRubric {
    BindRubric::new(bind_command_proto(binding), rubric_version, judge_scope).unwrap()
}

#[test]
fn binds_a_rubric_version_with_a_distinct_judge() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(bind_command())
        .then([bound()]);
}

#[test]
fn binds_to_a_node_subject_when_the_judge_is_proven_outside_it() {
    let binding = binding_with(BINDING_ID, node_subject("node-1"), JUDGE);

    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding.clone(),
            RubricVersionStanding::Available,
            JudgeScope::OutsideSubject,
        ))
        .then([v1::EvaluationBindingEvent {
            event: Some(
                v1::RubricBound {
                    binding: binding.into(),
                }
                .into(),
            ),
        }]);
}

#[test]
fn refuses_a_binding_that_already_exists() {
    TestCase::<BindRubric>::new()
        .given([bound()])
        .when(bind_command())
        .then_error(EvaluationBindingDecideError::AlreadyExists);
}

#[test]
fn refuses_to_rebind_under_the_id_of_an_unbound_binding() {
    TestCase::<BindRubric>::new()
        .given([bound(), unbound()])
        .when(bind_command())
        .then_error(EvaluationBindingDecideError::AlreadyExists);
}

#[test]
fn refuses_a_rubric_version_that_does_not_exist() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding(),
            RubricVersionStanding::Unknown,
            JudgeScope::OutsideSubject,
        ))
        .then_error(EvaluationBindingDecideError::RubricVersionNotFound);
}

#[test]
fn refuses_a_rubric_version_on_a_retired_rubric() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding(),
            RubricVersionStanding::Retired,
            JudgeScope::OutsideSubject,
        ))
        .then_error(EvaluationBindingDecideError::RubricVersionNotFound);
}

#[test]
fn refuses_a_judge_that_is_the_subject_agent() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding_with(BINDING_ID, agent_subject(JUDGE), JUDGE),
            RubricVersionStanding::Available,
            JudgeScope::OutsideSubject,
        ))
        .then_error(EvaluationBindingDecideError::JudgeMatchesSubject);
}

#[test]
fn refuses_a_judge_proven_to_sit_inside_the_subject_node() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding_with(BINDING_ID, node_subject("node-1"), JUDGE),
            RubricVersionStanding::Available,
            JudgeScope::InsideSubject,
        ))
        .then_error(EvaluationBindingDecideError::JudgeMatchesSubject);
}

#[test]
fn reports_a_missing_rubric_version_before_a_judge_conflict() {
    TestCase::<BindRubric>::new()
        .given_no_history()
        .when(command(
            binding_with(BINDING_ID, agent_subject(JUDGE), JUDGE),
            RubricVersionStanding::Unknown,
            JudgeScope::OutsideSubject,
        ))
        .then_error(EvaluationBindingDecideError::RubricVersionNotFound);
}

#[test]
fn rejects_commands_that_miss_required_fields() {
    let build = |binding: Option<v1::EvaluationBinding>| {
        BindRubric::new(
            v1::BindRubric {
                binding: binding.map(Into::into).unwrap_or_default(),
            },
            RubricVersionStanding::Available,
            JudgeScope::OutsideSubject,
        )
    };

    assert_eq!(build(None).unwrap_err().field, "binding");

    let mut no_id = binding();
    no_id.binding_id = Default::default();
    assert_eq!(build(Some(no_id)).unwrap_err().field, "binding.binding_id");

    let mut empty_id = binding();
    empty_id.binding_id = v1::EvaluationBindingId { value: String::new() }.into();
    assert_eq!(build(Some(empty_id)).unwrap_err().field, "binding.binding_id");

    let mut no_rubric = binding();
    no_rubric.rubric_version_ref = Default::default();
    assert_eq!(build(Some(no_rubric)).unwrap_err().field, "binding.rubric_version_ref");

    let mut no_version = binding();
    no_version.rubric_version_ref = trogonai_proto::evaluation::rubrics::v1alpha1::RubricVersionRef {
        rubric_id: trogonai_proto::evaluation::rubrics::v1alpha1::RubricId { value: "r".into() }.into(),
        version: String::new(),
    }
    .into();
    assert_eq!(build(Some(no_version)).unwrap_err().field, "binding.rubric_version_ref");

    let mut no_subject = binding();
    no_subject.subject = Default::default();
    assert_eq!(build(Some(no_subject)).unwrap_err().field, "binding.subject");

    let mut no_judge = binding();
    no_judge.judge = Default::default();
    assert_eq!(build(Some(no_judge)).unwrap_err().field, "binding.judge");
}

#[test]
fn targets_the_binding_stream_and_requires_it_to_be_new() {
    let command = bind_command();

    assert_eq!(command.binding_id(), BINDING_ID);
    assert_eq!(BindRubric::stream_id(&command), BINDING_ID);
    assert_eq!(BindRubric::WRITE_PRECONDITION, WritePrecondition::NoStream);
}

#[test]
fn refusals_display_the_fault_messages_without_naming_the_rubric() {
    assert_eq!(
        EvaluationBindingDecideError::AlreadyExists.to_string(),
        "The evaluation binding already exists."
    );
    assert_eq!(
        EvaluationBindingDecideError::RubricVersionNotFound.to_string(),
        "The rubric version named by the binding does not exist or its rubric is retired."
    );
    assert_eq!(
        EvaluationBindingDecideError::JudgeMatchesSubject.to_string(),
        "The judge must be distinct from every agent it evaluates."
    );
    assert_eq!(
        CommandInvalidError { field: "binding" }.to_string(),
        "The command violates the evaluation binding contract."
    );
}
