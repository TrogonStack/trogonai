use super::*;
use crate::commands::domain::ScheduleId;
use crate::config::ScheduleWriteCondition;
use trogon_decider_runtime::StreamPosition;

#[path = "../../tests/support/nats.rs"]
mod nats_support;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[test]
fn write_condition_rejects_unexpected_position() {
    let error = ScheduleWriteCondition::MustBeAtPosition(position(3))
        .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
        .unwrap_err();

    assert!(matches!(
        error,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(_),
            ..
        }
    ));
}

#[test]
fn new_streams_use_canonical_event_subject() {
    let state = resolve_event_subject_state(None);

    assert_eq!(state.write_state.current_position(), None);
    assert!(!state.write_state.exists());
}

#[test]
fn deleted_streams_keep_their_subject_and_still_count_as_existing() {
    let state = resolve_event_subject_state(Some(ScheduleWriteState::new(Some(position(12)), true)));

    assert_eq!(state.write_state.current_position(), Some(position(12)));
    assert!(state.write_state.exists());
}

#[test]
fn event_subject_uses_the_schedule_id() {
    let id = ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap();
    assert_eq!(event_subject(&id), format!("{EVENTS_SUBJECT_PREFIX}{id}"));
    assert!(event_subject(&id).ends_with(&id.to_string()));
}

#[tokio::test]
async fn stream_validation_refuses_missing_atomicity_routing_or_deduplication() {
    let (_server, client) = nats_support::start().await;
    let js = jetstream::new(client);
    let stream = js
        .create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_string(),
            subjects: vec![EVENTS_SUBJECT_PATTERN.to_string()],
            ..Default::default()
        })
        .await
        .unwrap();
    let error = validate_events_stream(&stream).unwrap_err();
    assert!(matches!(
        error,
        SchedulerError::Event {
            context: "events stream is missing allow_atomic",
            ..
        }
    ));

    let mut config = stream.cached_info().config.clone();
    config.allow_atomic_publish = true;
    config.subjects = vec!["other.events.>".to_string()];
    js.update_stream(config.clone()).await.unwrap();
    let unrouted = js.get_stream(EVENTS_STREAM).await.unwrap();
    let error = validate_events_stream(&unrouted).unwrap_err();
    assert!(matches!(
        error,
        SchedulerError::Event {
            context: "events stream is missing canonical schedule event subject coverage",
            ..
        }
    ));

    config.subjects = vec![EVENTS_SUBJECT_PATTERN.to_string()];
    config.duplicate_window = std::time::Duration::from_secs(1);
    js.update_stream(config.clone()).await.unwrap();
    let short_window = js.get_stream(EVENTS_STREAM).await.unwrap();
    assert!(validate_events_stream(&short_window).is_err());

    config.duplicate_window = EVENTS_DUPLICATE_WINDOW.as_duration();
    js.update_stream(config).await.unwrap();
    let valid = js.get_stream(EVENTS_STREAM).await.unwrap();
    validate_events_stream(&valid).unwrap();
}
