use chrono::{TimeZone, Utc};

use super::*;
use crate::commands::domain::ScheduleId;

fn schedule_id(raw: &str) -> ScheduleId {
    ScheduleId::parse(raw).unwrap()
}

fn occurrence_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 15, 18, 0, 0).unwrap()
}

#[test]
fn encode_decode_round_trips() {
    let payload = RRuleWakeupPayload::new(schedule_id("orders/rrule"), occurrence_at());
    let bytes = payload.encode().unwrap();
    let decoded = RRuleWakeupPayload::decode(&bytes).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn decode_rejects_invalid_json() {
    let err = RRuleWakeupPayload::decode(b"not json").unwrap_err();
    assert!(matches!(err, RRuleWakeupPayloadDecodeError::Json { .. }));
}

#[test]
fn decode_rejects_invalid_schedule_id() {
    let err = RRuleWakeupPayload::decode(br#"{"schedule_id":"","occurrence_at":"2026-06-15T18:00:00Z"}"#).unwrap_err();
    assert!(matches!(err, RRuleWakeupPayloadDecodeError::ScheduleId { .. }));
}

#[test]
fn decode_rejects_invalid_occurrence_at() {
    let err =
        RRuleWakeupPayload::decode(br#"{"schedule_id":"orders/rrule","occurrence_at":"not-a-time"}"#).unwrap_err();
    assert!(matches!(err, RRuleWakeupPayloadDecodeError::OccurrenceAt { .. }));
}
