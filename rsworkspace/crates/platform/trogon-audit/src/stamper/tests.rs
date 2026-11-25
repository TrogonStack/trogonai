use std::time::{Duration, UNIX_EPOCH};

use crate::fixtures::stamper;

#[test]
fn mints_distinct_ids_at_the_clock_time() {
    let stamper = stamper();
    let first = stamper.stamp();
    let second = stamper.stamp();
    assert_ne!(first.id(), second.id());
    let at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert_eq!(first.occurred_at, at);
    assert_eq!(first.recorded_at, at);
}

#[test]
fn can_backdate_the_occurrence() {
    let earlier = UNIX_EPOCH + Duration::from_secs(5);
    let stamp = stamper().stamp().occurring_at(earlier);
    assert_eq!(stamp.occurred_at, earlier);
    assert_ne!(stamp.recorded_at, earlier);
}
