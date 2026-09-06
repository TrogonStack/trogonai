use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, MessageView};
use serde_json::{Value, json};

use super::{assert_json_codec, assert_malformed, assert_wire_codec};
use crate::content::v1alpha1::Content;
use crate::google::r#type::{DateTime, TimeZone};
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};

fn schedules() -> [Value; 4] {
    [
        json!({"at": {"at": "2026-01-01T00:00:00.123Z"}}),
        json!({"every": {"every": "60.500s"}}),
        json!({"cron": {"expr": "0 9 * * *", "timezone": {"id": "UTC"}}}),
        json!({"rrule": {
            "dtstart": "2026-01-01T00:00:00Z", "rrule": "FREQ=DAILY;COUNT=3",
            "timezone": {"id": "UTC", "version": "2025b"},
            "rdate": ["2026-01-05T00:00:00Z"], "exdate": ["2026-01-02T00:00:00Z"]
        }}),
    ]
}

fn delivery() -> Value {
    json!({"natsMessage": {
        "subject": "jobs.backup", "ttl": "30s",
        "source": {"latestFromSubject": {"subject": "jobs.backup.template"}}
    }})
}

fn message() -> Value {
    json!({
        "content": {"contentType": "application/octet-stream", "data": "AAEC/w=="},
        "headers": [{"name": "x-region", "value": "west"}, {"name": "x-attempt", "value": "2"}]
    })
}

#[test]
fn schedule_variants_keep_wire_and_json_contracts_across_storage_schemas() {
    for schedule in schedules() {
        assert_json_codec::<v1::Schedule>(schedule.clone());
        assert_json_codec::<checkpoints_v1::Schedule>(schedule.clone());
        assert_json_codec::<projections_v1::Schedule>(schedule);
    }
}

#[test]
fn delivery_and_binary_content_survive_all_scheduler_schemas() {
    assert_json_codec::<v1::Delivery>(delivery());
    assert_json_codec::<checkpoints_v1::Delivery>(delivery());
    assert_json_codec::<projections_v1::Delivery>(delivery());
    assert_json_codec::<v1::Message>(message());
    assert_json_codec::<checkpoints_v1::Message>(message());
    assert_json_codec::<projections_v1::Message>(message());
    let content = assert_json_codec::<Content>(json!({"data": "AAEC/w=="}));
    assert_eq!(content.encode_to_vec(), [0x12, 0x04, 0x00, 0x01, 0x02, 0xff]);
}

#[test]
fn scheduler_commands_and_projection_preserve_present_false_and_timestamps() {
    let schedule = schedules()[1].clone();
    assert_json_codec::<v1::CreateSchedule>(json!({
        "scheduleId": "backup", "status": {"scheduled": {}},
        "schedule": schedule, "delivery": delivery(), "message": message()
    }));
    for status in [json!({"scheduled": {}}), json!({"paused": {}})] {
        assert_json_codec::<v1::ScheduleStatus>(status.clone());
        assert_json_codec::<projections_v1::ScheduleStatus>(status.clone());
        assert_json_codec::<projections_v1::ScheduleProjection>(json!({
            "scheduleId": "backup", "status": status, "completed": false,
            "nextOccurrenceAt": "2026-01-02T00:00:00Z",
            "lastOccurrenceAt": "2026-01-01T00:00:00Z",
            "schedule": schedule, "delivery": delivery(), "message": message()
        }));
    }
    assert_json_codec::<v1::PauseSchedule>(json!({"scheduleId": "backup"}));
    assert_json_codec::<v1::ResumeSchedule>(json!({"scheduleId": "backup"}));
    assert_json_codec::<v1::RemoveSchedule>(json!({"scheduleId": "backup"}));
}

#[test]
fn checkpoint_and_state_keep_optional_zero_and_unknown_enum_values() {
    assert_json_codec::<checkpoints_v1::ScheduleCheckpoint>(json!({
        "scheduleId": "backup", "status": 123, "lastAppliedStreamPosition": "0",
        "lastAppliedEventId": "event-7", "lastOutcome": 123,
        "schedule": schedules()[0], "delivery": delivery(), "message": message()
    }));
    let state = assert_json_codec::<state_v1::State>(json!({
        "state": "STATE_VALUE_PRESENT_ENABLED", "lastOccurrenceAt": "2026-01-01T00:00:00Z",
        "lastOccurrenceSequence": "18446744073709551615", "schedule": schedules()[1],
        "pendingOccurrenceAt": "2026-01-02T00:00:00Z", "completed": false
    }));
    assert_eq!(state.last_occurrence_sequence, Some(u64::MAX));
    assert_json_codec::<state_v1::State>(json!({}));
    assert_json_codec::<state_v1::State>(json!({"completed": false, "lastOccurrenceSequence": "0"}));
}

#[test]
fn occurrence_events_keep_uint64_precision() {
    assert_json_codec::<v1::ScheduleOccurrenceScheduled>(json!({
        "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
        "occurrenceAt": "2026-01-01T00:00:00Z", "scheduledAt": "2025-12-31T23:59:59Z"
    }));
    let event = json!({
        "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
        "occurrenceAt": "2026-01-01T00:00:00Z", "recordedAt": "2026-01-01T00:00:01Z"
    });
    assert_json_codec::<v1::ScheduleOccurrenceRecorded>(event);
}

#[test]
fn lifecycle_event_envelope_preserves_all_variants_and_replaces_previous_event() {
    let events = [
        json!({"scheduleCreated": {
            "scheduleId": "backup", "status": {"scheduled": {}},
            "schedule": schedules()[1], "delivery": delivery(), "message": message()
        }}),
        json!({"schedulePaused": {"scheduleId": "backup"}}),
        json!({"scheduleResumed": {"scheduleId": "backup"}}),
        json!({"scheduleRemoved": {"scheduleId": "backup"}}),
        json!({"scheduleOccurrenceScheduled": {
            "scheduleId": "backup", "occurrenceSequence": "7",
            "occurrenceAt": "2026-01-01T00:00:00Z", "scheduledAt": "2025-12-31T23:59:59Z"
        }}),
        json!({"scheduleOccurrenceRecorded": {
            "scheduleId": "backup", "occurrenceSequence": "7",
            "occurrenceAt": "2026-01-01T00:00:00Z", "recordedAt": "2026-01-01T00:00:01Z"
        }}),
        json!({"scheduleCompleted": {"scheduleId": "backup", "lastOccurrenceSequence": "7"}}),
    ];
    let mut wire = Vec::new();
    for event in events {
        let expected = assert_json_codec::<v1::ScheduleEvent>(event);
        wire.extend_from_slice(&expected.encode_to_vec());
        assert_wire_codec(&wire, &expected);
    }
}

#[test]
fn oneof_wire_uses_last_variant_but_merges_repeated_same_variant_messages() {
    let first = assert_json_codec::<v1::Schedule>(schedules()[0].clone());
    let second = assert_json_codec::<v1::Schedule>(schedules()[1].clone());
    let wire = [first.encode_to_vec(), second.encode_to_vec()].concat();
    assert_wire_codec(&wire, &second);

    let merged: v1::Schedule = serde_json::from_value(json!({
        "cron": {"expr": "old", "timezone": {"id": "UTC"}}
    }))
    .expect("merged schedule");
    assert_wire_codec(b"\x1a\x05\x0a\x03old\x1a\x07\x12\x05\x0a\x03UTC", &merged);
}

#[test]
fn nested_malformed_messages_fail_before_view_conversion() {
    assert_malformed::<v1::Schedule>(b"\x1a\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<v1::Schedule>(b"\x1a\x04\x0a\x03x", DecodeError::UnexpectedEof);
    assert_malformed::<Content>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<Content>(b"\x12\x04x", DecodeError::UnexpectedEof);
}

#[test]
fn decoding_limits_apply_to_nested_scheduler_views() {
    let schedule: v1::Schedule = serde_json::from_value(schedules()[0].clone()).expect("schedule");
    let wire = schedule.encode_to_vec();
    let too_small = DecodeOptions::new().with_max_message_size(wire.len() - 1);
    assert_eq!(
        v1::Schedule::decode_view_with_options(&wire, &too_small).err(),
        Some(DecodeError::MessageTooLarge)
    );
    let shallow = DecodeOptions::new().with_recursion_limit(1);
    assert_eq!(
        v1::Schedule::decode_view_with_options(&wire, &shallow).err(),
        Some(DecodeError::RecursionLimitExceeded)
    );
    let enough = DecodeOptions::new().with_recursion_limit(8);
    let view = v1::Schedule::decode_view_with_options(&wire, &enough).expect("sufficient depth");
    assert_eq!(view.to_owned_message().expect("view conversion"), schedule);
}

#[test]
fn civil_datetime_preserves_offset_oneof_and_subsecond_precision() {
    assert_json_codec::<TimeZone>(json!({"id": "Europe/Paris", "version": "2025b"}));
    for offset in [
        json!({"utcOffset": "3600s"}),
        json!({"timeZone": {"id": "Europe/Paris"}}),
    ] {
        let mut date =
            json!({"year": 2026, "month": 1, "day": 2, "hours": 3, "minutes": 4, "seconds": 5, "nanos": 123456789});
        date.as_object_mut()
            .expect("datetime object")
            .extend(offset.as_object().expect("offset object").clone());
        assert_json_codec::<DateTime>(date);
    }
}
