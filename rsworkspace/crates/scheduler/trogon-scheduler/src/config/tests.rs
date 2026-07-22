use super::*;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[test]
fn write_condition_ensures_expected_positions() {
    ScheduleWriteCondition::MustNotExist
        .ensure("alpha", ScheduleWriteState::new(None, false))
        .unwrap();
    ScheduleWriteCondition::MustBeAtPosition(position(3))
        .ensure("alpha", ScheduleWriteState::new(Some(position(3)), true))
        .unwrap();

    let error = ScheduleWriteCondition::MustBeAtPosition(position(2))
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
fn write_condition_rejects_reusing_deleted_stream_ids() {
    let error = ScheduleWriteCondition::MustNotExist
        .ensure("alpha", ScheduleWriteState::new(Some(position(7)), true))
        .unwrap_err();

    assert!(matches!(
        error,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(_),
            ..
        }
    ));
}
