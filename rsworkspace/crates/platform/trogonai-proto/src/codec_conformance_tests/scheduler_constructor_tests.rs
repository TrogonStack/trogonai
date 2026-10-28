use buffa::Message;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::assert_wire_codec;
use crate::scheduler::schedules::{projections_v1, v1};

fn assert_event_constructor<P: Message + DeserializeOwned>(payload_json: Value, variant: &str, tag: u8)
where
    Option<v1::schedule_event::Event>: From<P>,
{
    let payload: P = serde_json::from_value(payload_json.clone()).expect("event payload fixture");
    let payload_wire = payload.encode_to_vec();
    let event = v1::ScheduleEvent { event: payload.into() };
    let expected_json = Value::Object(Map::from_iter([(variant.to_owned(), payload_json)]));
    assert_eq!(
        serde_json::to_value(&event).expect("promoted event JSON"),
        expected_json
    );
    let mut expected_wire = vec![tag];
    buffa::encoding::encode_varint(
        u64::try_from(payload_wire.len()).expect("payload size"),
        &mut expected_wire,
    );
    expected_wire.extend_from_slice(&payload_wire);
    assert_eq!(event.encode_to_vec(), expected_wire);
    assert_wire_codec(&expected_wire, &event);
}

#[test]
fn lifecycle_payload_constructors_preserve_event_numbers_and_payload_fields() {
    assert_event_constructor::<v1::ScheduleCreated>(
        json!({
            "scheduleId": "backup", "status": {"scheduled": {}},
            "schedule": {"every": {"every": "60s"}},
            "delivery": {"natsMessage": {"subject": "jobs.backup"}},
            "message": {"content": {"contentType": "text/plain", "data": "aGk="}}
        }),
        "scheduleCreated",
        0x0a,
    );
    assert_event_constructor::<v1::SchedulePaused>(json!({"scheduleId": "backup"}), "schedulePaused", 0x12);
    assert_event_constructor::<v1::ScheduleResumed>(json!({"scheduleId": "backup"}), "scheduleResumed", 0x1a);
    assert_event_constructor::<v1::ScheduleRemoved>(json!({"scheduleId": "backup"}), "scheduleRemoved", 0x22);
    assert_event_constructor::<v1::ScheduleOccurrenceRecorded>(
        json!({
            "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
            "occurrenceAt": "1970-01-01T00:00:10Z", "recordedAt": "1970-01-01T00:00:11Z"
        }),
        "scheduleOccurrenceRecorded",
        0x2a,
    );
    assert_event_constructor::<v1::ScheduleOccurrenceScheduled>(
        json!({
            "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
            "occurrenceAt": "1970-01-01T00:00:10Z", "scheduledAt": "1970-01-01T00:00:09Z"
        }),
        "scheduleOccurrenceScheduled",
        0x32,
    );
    assert_event_constructor::<v1::ScheduleCompleted>(
        json!({"scheduleId": "backup", "lastOccurrenceSequence": "9007199254740993"}),
        "scheduleCompleted",
        0x3a,
    );
}

macro_rules! status_constructor_contracts {
    ($name:ident, $schema:ident) => {
        #[test]
        fn $name() {
            for (kind, expected_json, expected_wire) in [
                (Option::<$schema::schedule_status::Kind>::from($schema::schedule_status::Scheduled {}), json!({"scheduled": {}}), b"\x0a\x00"),
                (Option::<$schema::schedule_status::Kind>::from($schema::schedule_status::Paused {}), json!({"paused": {}}), b"\x12\x00"),
            ] {
                let status = $schema::ScheduleStatus { kind };
                assert_ne!(status, $schema::ScheduleStatus::default());
                assert_eq!(serde_json::to_value(&status).expect("selected status JSON"), expected_json);
                assert_eq!(status.encode_to_vec(), expected_wire);
                assert_wire_codec(expected_wire, &status);
            }
        }
    };
}

status_constructor_contracts!(live_status_constructors_preserve_presence_and_variant_numbers, v1);
status_constructor_contracts!(
    projection_status_constructors_preserve_presence_and_variant_numbers,
    projections_v1
);

macro_rules! check_sequence {
    ($message:ident, $setter:ident, $field:ident, $json_field:literal) => {
        for sequence in [0, 42, u64::MAX] {
            let absent = v1::$message {
                schedule_id: "backup".to_owned(),
                ..Default::default()
            };
            let present = absent.clone().$setter(sequence);
            let mut expected = absent.clone();
            expected.$field = Some(sequence);
            assert_eq!(present, expected);
            assert_ne!(present, absent);
            assert_eq!(
                serde_json::to_value(&present).expect("sequence JSON")[$json_field],
                json!(sequence.to_string())
            );
            let mut wire = absent.encode_to_vec();
            wire.push(0x10);
            buffa::encoding::encode_varint(sequence, &mut wire);
            assert_eq!(present.encode_to_vec(), wire);
            assert_wire_codec(&wire, &present);
        }
    };
}

#[test]
fn occurrence_builders_preserve_explicit_zero_and_existing_payload() {
    check_sequence!(
        ScheduleCompleted,
        with_last_occurrence_sequence,
        last_occurrence_sequence,
        "lastOccurrenceSequence"
    );
    check_sequence!(
        ScheduleOccurrenceRecorded,
        with_occurrence_sequence,
        occurrence_sequence,
        "occurrenceSequence"
    );
    check_sequence!(
        ScheduleOccurrenceScheduled,
        with_occurrence_sequence,
        occurrence_sequence,
        "occurrenceSequence"
    );
}

#[test]
fn completion_builder_keeps_false_present_and_preserves_projection_fields() {
    let source = json!({
        "scheduleId": "backup", "status": {"scheduled": {}},
        "schedule": {"every": {"every": "60s"}},
        "delivery": {"natsMessage": {"subject": "jobs.backup"}}, "message": {"content": {}}
    });
    let projection: projections_v1::ScheduleProjection = serde_json::from_value(source.clone()).expect("projection");
    for completed in [false, true] {
        let changed = projection.clone().with_completed(completed);
        let mut expected = source.clone();
        expected["completed"] = json!(completed);
        assert_eq!(serde_json::to_value(&changed).expect("completed projection"), expected);
        assert_ne!(changed, projection);
        let mut wire = projection.encode_to_vec();
        wire.extend_from_slice(&[0x18, u8::from(completed)]);
        assert_wire_codec(&wire, &changed);
    }
}
