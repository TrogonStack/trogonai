use std::num::NonZeroU32;

use trogonai_proto::agents::agents::v1alpha1::{AgentId, PlatformComponent};
use trogonai_proto::evaluation::bindings::v1alpha1::{
    EvaluationBinding, EvaluationBindingEvent, EvaluationBindingId, EvaluationResultId, EvaluationResultRecorded,
    Finding, FindingTarget, RubricBound, RubricUnbound,
};
use trogonai_proto::evaluation::bindings::{EvaluationBindingEventCase, FindingTargetCase};
use trogonai_proto::evaluation::rubrics::v1alpha1::{RubricId, RubricVersionRef};
use trogonai_proto::session::sessions::v1alpha1::{SessionId, SessionPositionRef};

use super::{FindingRecurrenceProjection, RecurrenceTarget, RubricKey};

fn rubric_ref(rubric: &str, version: &str) -> RubricVersionRef {
    RubricVersionRef {
        rubric_id: RubricId {
            value: rubric.to_string(),
        }
        .into(),
        version: version.to_string(),
    }
}

fn event(case: EvaluationBindingEventCase) -> EvaluationBindingEvent {
    EvaluationBindingEvent { event: Some(case) }
}

fn bound(binding: &str, rubric: &str, version: &str) -> EvaluationBindingEvent {
    event(
        RubricBound {
            binding: EvaluationBinding {
                binding_id: EvaluationBindingId {
                    value: binding.to_string(),
                }
                .into(),
                rubric_version_ref: rubric_ref(rubric, version).into(),
                judge: AgentId {
                    value: "judge".to_string(),
                }
                .into(),
                ..Default::default()
            }
            .into(),
        }
        .into(),
    )
}

fn path_finding(category: &str, path: &str) -> Finding {
    Finding {
        category: category.to_string(),
        target: FindingTarget {
            target: Some(FindingTargetCase::SettingsPath(path.to_string())),
        }
        .into(),
        ..Default::default()
    }
}

fn component_finding(category: &str, component: PlatformComponent) -> Finding {
    Finding {
        category: category.to_string(),
        target: FindingTarget {
            target: Some(FindingTargetCase::PlatformComponent(component.into())),
        }
        .into(),
        ..Default::default()
    }
}

fn recorded_raw(binding: &str, result: &str, session: &str, findings: Vec<Finding>) -> EvaluationResultRecorded {
    EvaluationResultRecorded {
        evaluation_result_id: EvaluationResultId {
            value: result.to_string(),
        }
        .into(),
        binding_id: EvaluationBindingId {
            value: binding.to_string(),
        }
        .into(),
        evaluated_ref: SessionPositionRef {
            session_id: SessionId {
                value: session.to_string(),
            }
            .into(),
            ..Default::default()
        }
        .into(),
        findings,
        ..Default::default()
    }
}

fn recorded(binding: &str, result: &str, session: &str, findings: Vec<Finding>) -> EvaluationBindingEvent {
    event(recorded_raw(binding, result, session, findings).into())
}

fn key() -> RubricKey {
    RubricKey::new("rubric-1", "v1")
}

fn fold_all(events: Vec<EvaluationBindingEvent>) -> FindingRecurrenceProjection {
    let mut projection = FindingRecurrenceProjection::new();
    for event in &events {
        projection.fold(event);
    }
    projection
}

#[test]
fn empty_projection_ranks_nothing() {
    assert!(FindingRecurrenceProjection::new().ranked(&key()).is_empty());
}

#[test]
fn counts_distinct_runs_and_results_separately() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        recorded("b1", "r1", "s1", vec![path_finding("tone", "a.b")]),
        recorded("b1", "r2", "s1", vec![path_finding("tone", "a.b")]),
        recorded("b1", "r3", "s2", vec![path_finding("tone", "a.b")]),
    ]);

    let rows = projection.ranked(&key());

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].category, "tone");
    assert_eq!(rows[0].target, RecurrenceTarget::SettingsPath("a.b".to_string()));
    assert_eq!(rows[0].distinct_run_count, 2);
    assert_eq!(rows[0].result_count, 3);
}

#[test]
fn a_finding_repeated_inside_one_result_counts_once() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        recorded(
            "b1",
            "r1",
            "s1",
            vec![path_finding("tone", "a.b"), path_finding("tone", "a.b")],
        ),
    ]);

    let rows = projection.ranked(&key());

    assert_eq!(rows[0].distinct_run_count, 1);
    assert_eq!(rows[0].result_count, 1);
}

#[test]
fn replaying_a_result_does_not_inflate_counts() {
    let result = recorded("b1", "r1", "s1", vec![path_finding("tone", "a.b")]);
    let projection = fold_all(vec![bound("b1", "rubric-1", "v1"), result.clone(), result]);

    assert_eq!(projection.ranked(&key())[0].result_count, 1);
}

#[test]
fn counts_are_scoped_to_the_rubric_version() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        bound("b2", "rubric-1", "v2"),
        recorded("b1", "r1", "s1", vec![path_finding("tone", "a.b")]),
        recorded("b2", "r2", "s2", vec![path_finding("tone", "a.b")]),
        recorded("b2", "r3", "s3", vec![path_finding("tone", "a.b")]),
    ]);

    assert_eq!(projection.ranked(&key())[0].distinct_run_count, 1);
    assert_eq!(
        projection.ranked(&RubricKey::new("rubric-1", "v2"))[0].distinct_run_count,
        2
    );
    assert!(projection.ranked(&RubricKey::new("rubric-2", "v1")).is_empty());
}

#[test]
fn results_before_the_unbind_remain_counted() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        recorded("b1", "r1", "s1", vec![path_finding("tone", "a.b")]),
        event(
            RubricUnbound {
                binding_id: EvaluationBindingId {
                    value: "b1".to_string(),
                }
                .into(),
            }
            .into(),
        ),
    ]);

    assert_eq!(projection.ranked(&key()).len(), 1);
}

#[test]
fn skips_results_whose_binding_is_unknown() {
    let projection = fold_all(vec![recorded("ghost", "r1", "s1", vec![path_finding("tone", "a.b")])]);

    assert!(projection.ranked(&key()).is_empty());
}

#[test]
fn skips_malformed_inputs_without_failing() {
    let no_binding = RubricBound::default();
    let no_ids = RubricBound {
        binding: EvaluationBinding::default().into(),
    };
    let projection = fold_all(vec![
        event(no_binding.into()),
        event(no_ids.into()),
        EvaluationBindingEvent { event: None },
        bound("b1", "rubric-1", "v1"),
        recorded(
            "b1",
            "r1",
            "s1",
            vec![Finding {
                category: "untargeted".to_string(),
                ..Default::default()
            }],
        ),
        recorded(
            "b1",
            "r2",
            "s1",
            vec![Finding {
                category: "empty-target".to_string(),
                target: FindingTarget::default().into(),
                ..Default::default()
            }],
        ),
    ]);

    assert!(projection.ranked(&key()).is_empty());
}

#[test]
fn tolerates_absent_identifiers_with_empty_defaults() {
    let binding = EvaluationBinding {
        binding_id: EvaluationBindingId {
            value: "b1".to_string(),
        }
        .into(),
        rubric_version_ref: RubricVersionRef {
            version: "v1".to_string(),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    };
    let mut no_ids = recorded_raw("b1", "r1", "s1", vec![path_finding("tone", "a.b")]);
    no_ids.evaluation_result_id = Default::default();
    no_ids.evaluated_ref = Default::default();
    let mut no_session = recorded_raw("b1", "r2", "s1", vec![path_finding("tone", "a.b")]);
    no_session.evaluated_ref = SessionPositionRef::default().into();
    let projection = fold_all(vec![
        event(
            RubricBound {
                binding: binding.into(),
            }
            .into(),
        ),
        event(no_ids.into()),
        event(no_session.into()),
    ]);

    let rows = projection.ranked(&RubricKey::new("", "v1"));

    assert_eq!(rows[0].distinct_run_count, 1);
    assert_eq!(rows[0].result_count, 2);
}

#[test]
fn ranks_by_runs_then_results_then_category_then_target() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        recorded(
            "b1",
            "r1",
            "s1",
            vec![
                path_finding("zeta", "a"),
                path_finding("alpha", "z"),
                path_finding("alpha", "a"),
                component_finding("alpha", PlatformComponent::PLATFORM_COMPONENT_MEMORY_KINDS),
                component_finding("alpha", PlatformComponent::PLATFORM_COMPONENT_SKILL_SET),
                path_finding("top", "a"),
                path_finding("rejudged", "a"),
            ],
        ),
        recorded("b1", "r2", "s2", vec![path_finding("top", "a")]),
        recorded("b1", "r3", "s1", vec![path_finding("rejudged", "a")]),
    ]);

    let order: Vec<(String, RecurrenceTarget)> = projection
        .ranked(&key())
        .into_iter()
        .map(|row| (row.category, row.target))
        .collect();

    let path = |value: &str| RecurrenceTarget::SettingsPath(value.to_string());
    assert_eq!(
        order,
        vec![
            ("top".to_string(), path("a")),
            ("rejudged".to_string(), path("a")),
            ("alpha".to_string(), path("a")),
            ("alpha".to_string(), path("z")),
            (
                "alpha".to_string(),
                RecurrenceTarget::PlatformComponent(PlatformComponent::PLATFORM_COMPONENT_SKILL_SET.into())
            ),
            (
                "alpha".to_string(),
                RecurrenceTarget::PlatformComponent(PlatformComponent::PLATFORM_COMPONENT_MEMORY_KINDS.into())
            ),
            ("zeta".to_string(), path("a")),
        ]
    );
}

#[test]
fn pages_through_the_ranking() {
    let projection = fold_all(vec![
        bound("b1", "rubric-1", "v1"),
        recorded(
            "b1",
            "r1",
            "s1",
            vec![path_finding("a", "p"), path_finding("b", "p"), path_finding("c", "p")],
        ),
    ]);
    let size = NonZeroU32::new(2).unwrap();

    let first = projection.page(&key(), size, 0);
    let second = projection.page(&key(), size, first.next_offset.unwrap());

    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_offset, Some(2));
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next_offset, None);
    assert!(projection.page(&key(), size, 10).items.is_empty());
}
