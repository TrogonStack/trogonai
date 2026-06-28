use buffa::{Message as _, MessageField, MessageName as _};

use crate::scheduler::schedules::v1::ScheduleOccurrenceRecorded;

fn timestamp(seconds: i64) -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp {
        seconds,
        nanos: 0,
        ..buffa_types::google::protobuf::Timestamp::default()
    }
}

#[test]
fn decode_event_to_json_is_canonical_across_wire_orderings() {
    let event = ScheduleOccurrenceRecorded {
        schedule_id: "backup".to_string(),
        occurrence_sequence: Some(7),
        occurrence_at: MessageField::some(timestamp(1_700_000_000)),
        recorded_at: MessageField::some(timestamp(1_700_000_005)),
    };
    let canonical = event.encode_to_vec();

    // Re-encode the same message with field 2 (occurrence_sequence) emitted
    // before field 1; protobuf permits any field order, so this is a valid
    // alternate encoding that differs on the wire. Build each fragment with the
    // encoder rather than hand-rolling tags.
    let only_sequence = ScheduleOccurrenceRecorded {
        occurrence_sequence: Some(7),
        ..ScheduleOccurrenceRecorded::default()
    }
    .encode_to_vec();
    let without_sequence = ScheduleOccurrenceRecorded {
        occurrence_sequence: None,
        ..event.clone()
    }
    .encode_to_vec();
    let reordered = [only_sequence, without_sequence].concat();
    assert_ne!(canonical, reordered, "encodings must differ on the wire");

    let from_canonical = super::decode_event_to_json(ScheduleOccurrenceRecorded::FULL_NAME, &canonical);
    let from_reordered = super::decode_event_to_json(ScheduleOccurrenceRecorded::FULL_NAME, &reordered);

    assert!(from_canonical.is_some());
    assert_eq!(from_canonical, from_reordered);
}

#[test]
fn decode_event_to_json_returns_none_for_unknown_type() {
    assert_eq!(
        super::decode_event_to_json("type.googleapis.com/trogonai.scheduler.schedules.v1.Unknown", &[]),
        None
    );
}
