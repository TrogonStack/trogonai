use trogon_decider::testing::TestCase;
use trogonai_proto::agents::agents::v1alpha1::PlatformComponent;
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

use super::*;
use crate::facts::FindingEvidence;
use crate::fixtures::*;

fn clear(findings: Vec<v1::Finding>, paths: &[&str]) -> RecordEvaluationResult {
    record_command("result-1", findings, evidence(surface(paths, &[])))
}

#[test]
fn records_a_result_without_findings() {
    let proto = record_proto("result-1", vec![]);

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(vec![], &[]))
        .then([recorded_from(&proto)]);
}

#[test]
fn records_findings_inside_the_editable_surface() {
    let findings = vec![settings_finding("instructions")];
    let proto = record_proto("result-1", findings.clone());

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(findings, &["instructions"]))
        .then([recorded_from(&proto)]);
}

#[test]
fn passes_findings_coverage_and_judge_session_ref_through_to_the_event() {
    let findings = vec![settings_finding("instructions"), settings_finding("tools.search")];
    let mut proto = record_proto("result-1", findings.clone());
    proto.coverage = coverage().into();
    proto.judge_session_ref = session_position("session-judge", 3).into();

    let command =
        RecordEvaluationResult::new(proto.clone(), evidence(surface(&["instructions", "tools"], &[]))).unwrap();

    let expected = recorded_from(&proto);
    let Some(v1::__buffa::oneof::evaluation_binding_event::Event::EvaluationResultRecorded(recorded)) =
        expected.event.clone()
    else {
        panic!("expected a recorded event");
    };
    assert_eq!(recorded.findings, findings);
    assert_eq!(recorded.coverage, proto.coverage);
    assert_eq!(recorded.judge_session_ref, proto.judge_session_ref);

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(command)
        .then([expected]);
}

#[test]
fn a_listed_path_covers_its_subtree() {
    let findings = vec![settings_finding("tools.search.timeout")];
    let proto = record_proto("result-1", findings.clone());

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(findings, &["tools"]))
        .then([recorded_from(&proto)]);
}

#[test]
fn a_listed_path_does_not_cover_a_sibling_sharing_its_prefix() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(vec![settings_finding("toolset")], &["tools"]))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn a_listed_path_does_not_cover_its_parent() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(vec![settings_finding("tools")], &["tools.search"]))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn records_a_platform_component_finding_listed_on_the_surface() {
    let component = PlatformComponent::default();
    let findings = vec![component_finding(component)];
    let proto = record_proto("result-1", findings.clone());

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            findings,
            evidence(surface(&[], &[component])),
        ))
        .then([recorded_from(&proto)]);
}

#[test]
fn refuses_a_platform_component_finding_missing_from_the_surface() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            vec![component_finding(PlatformComponent::default())],
            evidence(surface(&["instructions"], &[])),
        ))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn refuses_findings_when_the_implementation_opts_out_of_candidates() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            vec![settings_finding("instructions")],
            evidence(Default::default()),
        ))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn refuses_when_any_one_finding_is_outside_the_surface() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(clear(
            vec![settings_finding("instructions"), settings_finding("secrets")],
            &["instructions"],
        ))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn refuses_findings_that_match_the_withheld_vocabulary() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            vec![settings_finding("instructions")],
            FindingEvidence {
                editable_surface: surface(&["instructions"], &[]),
                withheld_vocabulary: WithheldVocabularyScreen::Matched,
            },
        ))
        .then_error(EvaluationBindingDecideError::FindingMatchesWithheldVocabulary);
}

#[test]
fn reports_the_surface_before_the_vocabulary() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            vec![settings_finding("secrets")],
            FindingEvidence {
                editable_surface: surface(&["instructions"], &[]),
                withheld_vocabulary: WithheldVocabularyScreen::Matched,
            },
        ))
        .then_error(EvaluationBindingDecideError::FindingOutsideEditableSurface);
}

#[test]
fn a_vocabulary_match_does_not_block_a_result_without_findings() {
    let proto = record_proto("result-1", vec![]);

    TestCase::<RecordEvaluationResult>::new()
        .given([bound()])
        .when(record_command(
            "result-1",
            vec![],
            FindingEvidence {
                editable_surface: Default::default(),
                withheld_vocabulary: WithheldVocabularyScreen::Matched,
            },
        ))
        .then([recorded_from(&proto)]);
}

#[test]
fn refuses_a_repeated_evaluation_result_id() {
    let proto = record_proto("result-1", vec![]);

    TestCase::<RecordEvaluationResult>::new()
        .given([bound(), recorded_from(&proto)])
        .when(clear(vec![], &[]))
        .then_error(EvaluationBindingDecideError::ResultAlreadyRecorded);
}

#[test]
fn records_a_second_result_under_a_new_id() {
    let first = record_proto("result-1", vec![]);
    let second = record_proto("result-2", vec![]);

    TestCase::<RecordEvaluationResult>::new()
        .given([bound(), recorded_from(&first)])
        .when(record_command("result-2", vec![], evidence(Default::default())))
        .then([recorded_from(&second)]);
}

#[test]
fn refuses_a_binding_that_does_not_exist() {
    TestCase::<RecordEvaluationResult>::new()
        .given_no_history()
        .when(clear(vec![], &[]))
        .then_error(EvaluationBindingDecideError::NotFound);
}

#[test]
fn refuses_a_binding_that_is_unbound() {
    TestCase::<RecordEvaluationResult>::new()
        .given([bound(), unbound()])
        .when(clear(vec![], &[]))
        .then_error(EvaluationBindingDecideError::NotFound);
}

#[test]
fn rejects_commands_that_miss_required_fields() {
    let evidence = || evidence(Default::default());
    let field = |proto: v1::RecordEvaluationResult| RecordEvaluationResult::new(proto, evidence()).unwrap_err().field;
    let base = || record_proto("result-1", vec![]);

    let mut p = base();
    p.evaluation_result_id = Default::default();
    assert_eq!(field(p), "evaluation_result_id");

    let mut p = base();
    p.evaluation_result_id = v1::EvaluationResultId { value: String::new() }.into();
    assert_eq!(field(p), "evaluation_result_id");

    let mut p = base();
    p.binding_id = Default::default();
    assert_eq!(field(p), "binding_id");

    let mut p = base();
    p.evaluated_ref = Default::default();
    assert_eq!(field(p), "evaluated_ref");

    let mut p = base();
    p.score = Default::default();
    assert_eq!(field(p), "score");

    let mut p = base();
    p.content_digest = Default::default();
    assert_eq!(field(p), "content_digest");

    let mut p = base();
    p.findings = (0..65).map(|_| settings_finding("instructions")).collect();
    assert_eq!(field(p), "findings");

    let mut p = base();
    let mut finding = settings_finding("instructions");
    finding.target = Default::default();
    p.findings = vec![finding];
    assert_eq!(field(p), "findings.target");
}

#[test]
fn targets_the_binding_stream_and_requires_it_to_exist_unchanged() {
    let command = clear(vec![], &[]);

    assert_eq!(command.binding_id(), BINDING_ID);
    assert_eq!(RecordEvaluationResult::stream_id(&command), BINDING_ID);
    assert_eq!(
        RecordEvaluationResult::WRITE_PRECONDITION,
        WritePrecondition::StreamUnchanged
    );
}

#[test]
fn refusals_display_the_fault_messages_without_naming_a_term_or_rubric() {
    let messages = [
        (
            EvaluationBindingDecideError::ResultAlreadyRecorded,
            "The evaluation result has already been recorded.",
        ),
        (
            EvaluationBindingDecideError::FindingOutsideEditableSurface,
            "A finding names a surface the evaluated revision does not allow a candidate to edit.",
        ),
        (
            EvaluationBindingDecideError::FindingMatchesWithheldVocabulary,
            "A finding matches vocabulary its rubric version withholds.",
        ),
    ];
    for (error, message) in messages {
        assert_eq!(error.to_string(), message);
    }
}
