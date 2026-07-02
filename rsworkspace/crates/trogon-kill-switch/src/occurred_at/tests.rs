use time::macros::datetime;

use super::*;

#[test]
fn round_trips_through_rfc3339() {
    let instant = OccurredAt::new(datetime!(2026-07-02 12:30:00 UTC));
    let formatted = instant.to_rfc3339().unwrap();
    assert_eq!(formatted, "2026-07-02T12:30:00Z");
    assert_eq!(OccurredAt::parse_rfc3339(&formatted).unwrap(), instant);
}

#[test]
fn rejects_malformed_rfc3339() {
    let error = OccurredAt::parse_rfc3339("not-a-timestamp").unwrap_err();
    assert!(matches!(error, OccurredAtError::Parse(_)));
}

#[test]
fn now_produces_a_utc_instant() {
    let before = OffsetDateTime::now_utc();
    let occurred_at = OccurredAt::now();
    let after = OffsetDateTime::now_utc();
    assert!(occurred_at.as_offset_date_time() >= before);
    assert!(occurred_at.as_offset_date_time() <= after);
}
