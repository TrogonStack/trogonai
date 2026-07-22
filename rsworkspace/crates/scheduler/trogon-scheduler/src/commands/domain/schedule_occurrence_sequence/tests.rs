use super::*;

#[test]
fn rejects_zero() {
    assert_eq!(
        ScheduleOccurrenceSequence::try_new(0).unwrap_err(),
        ScheduleOccurrenceSequenceError::Zero
    );
}

#[test]
fn advances_from_the_last_accepted_sequence() {
    assert_eq!(ScheduleOccurrenceSequence::next_after(41).unwrap().as_u64(), 42);
}

#[test]
fn displays_inner_sequence() {
    assert_eq!(ScheduleOccurrenceSequence::try_new(42).unwrap().to_string(), "42");
}

#[test]
fn reports_overflow() {
    assert_eq!(
        ScheduleOccurrenceSequence::next_after(u64::MAX).unwrap_err(),
        ScheduleOccurrenceSequenceError::Overflow
    );
}
