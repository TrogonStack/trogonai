use std::num::NonZeroU64;

use buffa::Message as _;
use chrono::DateTime;
use trogon_decider_nats::{CheckpointSequence, ProjectionApply, ProjectionCheckpointStore};
use trogon_decider_runtime::{Event, EventId, Headers, StreamEvent, StreamPosition};
use trogonai_proto::evaluation::bindings::FindingTargetCase;
use trogonai_proto::evaluation::bindings::v1alpha1::{
    EvaluationBinding, EvaluationBindingId, EvaluationResultId, EvaluationResultRecorded, Finding, FindingTarget,
    RubricBound, RubricUnbound,
};
use trogonai_proto::evaluation::rubrics::v1alpha1::{RubricId, RubricVersionRef};
use trogonai_proto::session::sessions::v1alpha1::{SessionId, SessionPositionRef};
use uuid::Uuid;

use super::{RebuildFromZero, RecurrenceApply};
use crate::projection::{FindingRecurrenceProjection, RubricKey};

fn stream_event(event_type: &str, content: Vec<u8>, position: u64) -> StreamEvent {
    StreamEvent {
        stream_id: "b1".to_string(),
        event: Event {
            id: EventId::new(Uuid::nil()),
            r#type: event_type.to_string(),
            content,
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::new(NonZeroU64::new(position).unwrap()),
        recorded_at: DateTime::UNIX_EPOCH,
    }
}

fn bound_event(position: u64) -> StreamEvent {
    let bound = RubricBound {
        binding: EvaluationBinding {
            binding_id: EvaluationBindingId {
                value: "b1".to_string(),
            }
            .into(),
            rubric_version_ref: RubricVersionRef {
                rubric_id: RubricId {
                    value: "rubric-1".to_string(),
                }
                .into(),
                version: "v1".to_string(),
            }
            .into(),
            ..Default::default()
        }
        .into(),
    };
    stream_event(
        "trogonai.evaluation.bindings.v1alpha1.RubricBound",
        bound.encode_to_vec(),
        position,
    )
}

fn recorded_event(position: u64) -> StreamEvent {
    let recorded = EvaluationResultRecorded {
        evaluation_result_id: EvaluationResultId {
            value: "r1".to_string(),
        }
        .into(),
        binding_id: EvaluationBindingId {
            value: "b1".to_string(),
        }
        .into(),
        evaluated_ref: SessionPositionRef {
            session_id: SessionId {
                value: "s1".to_string(),
            }
            .into(),
            ..Default::default()
        }
        .into(),
        findings: vec![Finding {
            category: "tone".to_string(),
            target: FindingTarget {
                target: Some(FindingTargetCase::SettingsPath("a.b".to_string())),
            }
            .into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    stream_event(
        "trogonai.evaluation.bindings.v1alpha1.EvaluationResultRecorded",
        recorded.encode_to_vec(),
        position,
    )
}

#[tokio::test]
async fn folds_bound_and_recorded_events_and_returns_their_position() {
    let mut projection = FindingRecurrenceProjection::new();
    let mut apply = RecurrenceApply::new(&mut projection);

    let first = apply.apply(bound_event(1)).await.unwrap();
    let second = apply.apply(recorded_event(2)).await.unwrap();

    assert_eq!(first, CheckpointSequence::new(1));
    assert_eq!(second, CheckpointSequence::new(2));
    let rows = projection.ranked(&RubricKey::new("rubric-1", "v1"));
    assert_eq!(rows[0].distinct_run_count, 1);
}

#[tokio::test]
async fn accepts_an_unbound_event_without_changing_counts() {
    let mut projection = FindingRecurrenceProjection::new();
    let unbound = RubricUnbound {
        binding_id: EvaluationBindingId {
            value: "b1".to_string(),
        }
        .into(),
    };
    let event = stream_event(
        "trogonai.evaluation.bindings.v1alpha1.RubricUnbound",
        unbound.encode_to_vec(),
        3,
    );

    let checkpoint = RecurrenceApply::new(&mut projection).apply(event).await.unwrap();

    assert_eq!(checkpoint, CheckpointSequence::new(3));
}

#[tokio::test]
async fn skips_event_types_from_other_sets() {
    let mut projection = FindingRecurrenceProjection::new();

    let checkpoint = RecurrenceApply::new(&mut projection)
        .apply(stream_event("other.Event", vec![0xff], 4))
        .await
        .unwrap();

    assert_eq!(checkpoint, CheckpointSequence::new(4));
}

#[tokio::test]
async fn fails_on_a_payload_that_claims_the_binding_event_set() {
    let mut projection = FindingRecurrenceProjection::new();

    let error = RecurrenceApply::new(&mut projection)
        .apply(stream_event(
            "trogonai.evaluation.bindings.v1alpha1.RubricBound",
            vec![0xff],
            5,
        ))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("failed to decode an evaluation binding event")
    );
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn rebuild_from_zero_never_remembers_progress() {
    let store = RebuildFromZero;

    store.save(CheckpointSequence::new(9)).await.unwrap();

    assert_eq!(store.load().await.unwrap(), CheckpointSequence::NONE);
}
