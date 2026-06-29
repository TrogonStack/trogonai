use super::*;
use crate::config::ScheduleWriteCondition;
use crate::projections::storage::read_model_key;
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
fn event_subject_uses_the_derived_routing_key() {
    // The final token is the derived key (so the catch-up fold can match events
    // back to their schedule), never the raw id.
    let id = "orders/created";
    assert_eq!(
        event_subject(id),
        format!("{EVENTS_SUBJECT_PREFIX}{}", read_model_key(id))
    );
    assert!(!event_subject(id).ends_with(id));
}
