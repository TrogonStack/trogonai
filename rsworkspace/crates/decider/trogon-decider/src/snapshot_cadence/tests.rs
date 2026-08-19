use super::SnapshotCadence;
use std::num::NonZeroU64;

fn frequency(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("non-zero frequency")
}

#[test]
fn a_zero_frequency_degrades_to_never_rather_than_panicking() {
    assert_eq!(SnapshotCadence::every_events(0), SnapshotCadence::Never);
    assert_eq!(
        SnapshotCadence::every_events(3),
        SnapshotCadence::EveryEvents(frequency(3))
    );
}

#[test]
fn never_reports_no_frequency_and_is_never_due() {
    assert_eq!(SnapshotCadence::Never.frequency(), None);
    assert!(!SnapshotCadence::Never.is_due(u64::MAX));
}

#[test]
fn every_events_is_due_once_the_frequency_is_reached() {
    let cadence = SnapshotCadence::EveryEvents(frequency(3));

    assert_eq!(cadence.frequency(), Some(frequency(3)));
    assert!(!cadence.is_due(2));
    assert!(cadence.is_due(3));
    assert!(cadence.is_due(4));
}
