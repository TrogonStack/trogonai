use super::*;
use crate::commands::domain::ScheduleId;
use crate::config::ScheduleWriteCondition;
use trogon_decider_runtime::StreamPosition;

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
