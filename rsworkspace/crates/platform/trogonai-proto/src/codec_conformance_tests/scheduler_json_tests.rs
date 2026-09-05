use std::fmt::Debug;

use buffa::{DecodeError, HasMessageView, Message};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{assert_json_codec, assert_malformed, assert_wire_codec};
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};

fn assert_normalized<M: DeserializeOwned + Serialize>(input: Value, expected: Value) {
    let parsed: M = serde_json::from_value(input).expect("accepted ProtoJSON");
    assert_eq!(serde_json::to_value(parsed).expect("canonical ProtoJSON"), expected);
}

fn assert_rejected<M: DeserializeOwned + Debug>(input: &str) {
    assert!(serde_json::from_str::<M>(input).is_err(), "accepted {input}");
}

fn assert_length_delimited_fields<M: Message + HasMessageView>(fields: &[u8]) {
    for &field in fields {
        assert_malformed::<M>(
            &[field << 3, 0],
            DecodeError::WireTypeMismatch {
                field_number: u32::from(field),
                expected: 2,
                actual: 0,
            },
        );
        assert_malformed::<M>(&[(field << 3) | 2, 2, 0], DecodeError::UnexpectedEof);
    }
}

macro_rules! shared_schema_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn null_and_unknown_oneof_fields_do_not_select_a_schedule() {
                assert_normalized::<schema::Schedule>(
                    json!({"at": null, "every": null, "cron": null, "rrule": null,
                        "futureStrategy": {"nested": [true, 1, {"x": "y"}]}}),
                    json!({}),
                );
                assert_normalized::<schema::Schedule>(
                    json!({"at": null, "every": {"every": "1s"}}),
                    json!({"every": {"every": "1s"}}),
                );
                assert_json_codec::<schema::Schedule>(json!({}));
            }

            #[test]
            fn conflicting_and_repeated_oneof_members_are_rejected() {
                for input in [
                    r#"{"at":{"at":"2026-01-01T00:00:00Z"},"every":{"every":"1s"}}"#,
                    r#"{"every":{"every":"1s"},"cron":{"expr":"* * * * *","timezone":{"id":"UTC"}}}"#,
                    r#"{"cron":{"expr":"* * * * *","timezone":{"id":"UTC"}},"rrule":{"dtstart":"2026-01-01T00:00:00Z","rrule":"FREQ=DAILY","timezone":{"id":"UTC"}}}"#,
                    r#"{"at":{"at":"2026-01-01T00:00:00Z"},"at":{"at":"2026-01-02T00:00:00Z"}}"#,
                    r#"{"every":{"every":"1s"},"every":{"every":"2s"}}"#,
                    r#"{"cron":{"expr":"a","timezone":{"id":"UTC"}},"cron":{"expr":"b","timezone":{"id":"UTC"}}}"#,
                    r#"{"rrule":{"dtstart":"2026-01-01T00:00:00Z","rrule":"FREQ=DAILY","timezone":{"id":"UTC"}},"rrule":{"dtstart":"2026-01-02T00:00:00Z","rrule":"FREQ=DAILY","timezone":{"id":"UTC"}}}"#,
                ] {
                    assert_rejected::<schema::Schedule>(input);
                }
            }

            #[test]
            fn delivery_aliases_normalize_and_null_source_is_absent() {
                assert_normalized::<schema::Delivery>(
                    json!({"nats_message": {"subject": "jobs.run", "ttl": null,
                        "source": {"latest_from_subject": {"subject": "jobs.template"}}}}),
                    json!({"natsMessage": {"subject": "jobs.run",
                        "source": {"latestFromSubject": {"subject": "jobs.template"}}}}),
                );
                assert_normalized::<schema::Delivery>(
                    json!({"natsMessage": {"subject": "jobs.run", "source": null}}),
                    json!({"natsMessage": {"subject": "jobs.run"}}),
                );
                assert_normalized::<schema::Delivery>(json!({"natsMessage": null}), json!({}));
                assert_normalized::<schema::delivery::nats_message::Source>(
                    json!({"latestFromSubject": null, "newSource": {"ids": [1, 2]}}),
                    json!({}),
                );
                assert_rejected::<schema::Delivery>(
                    r#"{"natsMessage":{"subject":"a"},"nats_message":{"subject":"b"}}"#,
                );
                assert_rejected::<schema::delivery::nats_message::Source>(
                    r#"{"latestFromSubject":{"subject":"a"},"latest_from_subject":{"subject":"b"}}"#,
                );
            }

            #[test]
            fn absent_and_empty_repeated_values_share_canonical_json() {
                for empty in [json!(null), json!([])] {
                    assert_normalized::<schema::schedule::RRule>(
                        json!({"dtstart": "2026-01-01T00:00:00Z", "rrule": "FREQ=DAILY",
                            "timezone": {"id": "UTC"}, "rdate": empty, "exdate": empty}),
                        json!({"dtstart": "2026-01-01T00:00:00Z", "rrule": "FREQ=DAILY",
                            "timezone": {"id": "UTC"}}),
                    );
                    assert_normalized::<schema::Message>(
                        json!({"content": {}, "headers": empty}),
                        json!({"content": {}}),
                    );
                }
                assert_rejected::<schema::Message>(r#"{"content":{},"headers":[null]}"#);
                assert_rejected::<schema::schedule::RRule>(
                    r#"{"dtstart":"2026-01-01T00:00:00Z","rrule":"FREQ=DAILY","timezone":{"id":"UTC"},"rdate":[null]}"#,
                );
            }

            #[test]
            fn repeated_message_fields_merge_without_losing_previous_values() {
                let expected: schema::Schedule = serde_json::from_value(json!({
                    "rrule": {"dtstart": "1970-01-01T00:00:00Z", "rrule": "FREQ=DAILY",
                        "timezone": {"id": "UTC"}, "rdate": ["1970-01-01T00:00:01Z", "1970-01-01T00:00:02Z"]}
                })).expect("recurrence");
                let first: schema::Schedule = serde_json::from_value(json!({
                    "rrule": {"dtstart": "1970-01-01T00:00:00Z", "rrule": "FREQ=DAILY",
                        "timezone": {"id": "UTC"}, "rdate": ["1970-01-01T00:00:01Z"]}
                })).expect("first recurrence fragment");
                let wire = [first.encode_to_vec(), vec![0x22, 0x04, 0x22, 0x02, 0x08, 0x02]].concat();
                assert_wire_codec(&wire, &expected);
                let expected: schema::Delivery = serde_json::from_value(json!({
                    "natsMessage": {"subject": "jobs.run", "ttl": "5s"}
                })).expect("merged delivery");
                assert_wire_codec(b"\x0a\x0a\x0a\x08jobs.run\x0a\x04\x12\x02\x08\x05", &expected);
            }

            #[test]
            fn duplicate_schedule_messages_merge_complementary_nested_fields() {
                for (wire, expected) in [
                    (
                        b"\x0a\x04\x0a\x02\x08\x01\x0a\x04\x0a\x02\x10\x02".as_slice(),
                        json!({"at": {"at": "1970-01-01T00:00:01.000000002Z"}}),
                    ),
                    (
                        b"\x12\x04\x0a\x02\x08\x01\x12\x04\x0a\x02\x10\x02".as_slice(),
                        json!({"every": {"every": "1.000000002s"}}),
                    ),
                    (
                        b"\x1a\x05\x0a\x03old\x1a\x07\x12\x05\x0a\x03UTC".as_slice(),
                        json!({"cron": {"expr": "old", "timezone": {"id": "UTC"}}}),
                    ),
                ] {
                    let expected: schema::Schedule = serde_json::from_value(expected).expect("merged definition");
                    assert_wire_codec(wire, &expected);
                }
                let wire = b"\x0a\x02\x08\x01\x12\x0aFREQ=DAILY\x1a\x05\x0a\x03UTC\x0a\x02\x10\x02\x1a\x07\x12\x052025b";
                let expected: schema::schedule::RRule = serde_json::from_value(json!({
                    "dtstart": "1970-01-01T00:00:01.000000002Z", "rrule": "FREQ=DAILY",
                    "timezone": {"id": "UTC", "version": "2025b"}
                })).expect("merged recurrence");
                assert_wire_codec(wire, &expected);
            }

            #[test]
            fn known_fields_reject_wrong_wire_types_and_truncated_values() {
                assert_length_delimited_fields::<schema::Schedule>(&[1, 2, 3, 4]);
                assert_length_delimited_fields::<schema::schedule::At>(&[1]);
                assert_length_delimited_fields::<schema::schedule::Every>(&[1]);
                assert_length_delimited_fields::<schema::schedule::Cron>(&[1, 2]);
                assert_length_delimited_fields::<schema::schedule::RRule>(&[1, 2, 3, 4, 5]);
                assert_length_delimited_fields::<schema::Delivery>(&[1]);
                assert_length_delimited_fields::<schema::delivery::NatsMessage>(&[1, 2, 3]);
                assert_length_delimited_fields::<schema::delivery::nats_message::Source>(&[1]);
                assert_length_delimited_fields::<schema::delivery::nats_message::LatestFromSubject>(&[1]);
                assert_length_delimited_fields::<schema::Message>(&[1, 2]);
                assert_length_delimited_fields::<schema::Header>(&[1, 2]);
            }

            #[test]
            fn oneof_payloads_reject_incompatible_json_values_and_ignore_future_fields() {
                for input in ["true", "1", "[]", r#""schedule""#,
                    r#"{"at":true}"#, r#"{"every":1}"#, r#"{"cron":true}"#, r#"{"rrule":"daily"}"#] {
                    assert_rejected::<schema::Schedule>(input);
                }
                for input in ["true", "[]", r#"{"natsMessage":1}"#] {
                    assert_rejected::<schema::Delivery>(input);
                }
                for input in ["false", "[]", r#"{"latestFromSubject":"subject"}"#] {
                    assert_rejected::<schema::delivery::nats_message::Source>(input);
                }
                assert_normalized::<schema::Delivery>(json!({"future": {"list": [true, {}]}}), json!({}));
                let expected: schema::delivery::nats_message::Source = serde_json::from_value(json!({
                    "latestFromSubject": {"subject": "last"}
                })).expect("last source subject");
                assert_wire_codec(b"\x0a\x07\x0a\x05first\x0a\x06\x0a\x04last", &expected);
            }

            #[test]
            fn malformed_leaf_and_nested_wire_is_rejected_consistently() {
                assert_malformed::<schema::Header>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::schedule::Cron>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::schedule::RRule>(b"\x12\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::delivery::NatsMessage>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::delivery::nats_message::LatestFromSubject>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::Message>(b"\x12\x03\x12\x01\xff", DecodeError::InvalidUtf8);
                assert_malformed::<schema::Schedule>(b"\x22\x04\x22\x03\x08", DecodeError::UnexpectedEof);
            }
        }
    };
}

shared_schema_contracts!(live_tests, v1);
shared_schema_contracts!(checkpoint_tests, checkpoints_v1);
shared_schema_contracts!(projection_tests, projections_v1);

#[test]
fn optional_storage_fields_accept_null_without_inventing_presence() {
    assert_normalized::<checkpoints_v1::ScheduleCheckpoint>(
        json!({
            "schedule_id": null, "status": null, "last_applied_stream_position": null,
            "last_applied_event_id": null, "last_outcome": null, "schedule": null,
            "delivery": null, "message": null, "futureField": {"nested": [1, 2]}
        }),
        json!({}),
    );
    assert_normalized::<state_v1::State>(
        json!({
            "state": null, "last_occurrence_at": null, "last_occurrence_sequence": null,
            "schedule": null, "pending_occurrence_at": null, "completed": null
        }),
        json!({}),
    );
    assert_normalized::<checkpoints_v1::ScheduleCheckpoint>(
        json!({
            "schedule_id": "job", "last_applied_stream_position": 42, "last_applied_event_id": "event",
            "last_outcome": 0, "status": 0
        }),
        json!({
            "scheduleId": "job", "lastAppliedStreamPosition": "42", "lastAppliedEventId": "event",
            "lastOutcome": "RECONCILE_OUTCOME_UNSPECIFIED", "status": "SCHEDULE_CHECKPOINT_STATUS_UNSPECIFIED"
        }),
    );
    assert_normalized::<state_v1::State>(
        json!({
            "state": 0, "last_occurrence_sequence": 0, "completed": false
        }),
        json!({"state": "STATE_VALUE_UNSPECIFIED", "lastOccurrenceSequence": "0", "completed": false}),
    );
}

#[test]
fn storage_enums_accept_known_names_and_preserve_unknown_numbers() {
    for status in [
        "SCHEDULE_CHECKPOINT_STATUS_UNSPECIFIED",
        "SCHEDULE_CHECKPOINT_STATUS_SCHEDULED",
        "SCHEDULE_CHECKPOINT_STATUS_PAUSED",
        "SCHEDULE_CHECKPOINT_STATUS_REMOVED",
        "SCHEDULE_CHECKPOINT_STATUS_UNSUPPORTED",
        "SCHEDULE_CHECKPOINT_STATUS_EXPIRED",
    ] {
        assert_json_codec::<checkpoints_v1::ScheduleCheckpoint>(json!({"status": status}));
    }
    for outcome in [
        "RECONCILE_OUTCOME_UNSPECIFIED",
        "RECONCILE_OUTCOME_PUBLISHED",
        "RECONCILE_OUTCOME_PURGED",
        "RECONCILE_OUTCOME_STORED_PAUSED",
        "RECONCILE_OUTCOME_UNSUPPORTED",
        "RECONCILE_OUTCOME_EXPIRED",
        "RECONCILE_OUTCOME_DUPLICATE_STALE",
    ] {
        assert_json_codec::<checkpoints_v1::ScheduleCheckpoint>(json!({"lastOutcome": outcome}));
    }
    for state in [
        "STATE_VALUE_UNSPECIFIED",
        "STATE_VALUE_MISSING",
        "STATE_VALUE_PRESENT_ENABLED",
        "STATE_VALUE_PRESENT_DISABLED",
        "STATE_VALUE_DELETED",
    ] {
        assert_json_codec::<state_v1::State>(json!({"state": state}));
    }
    for number in [i32::MIN, -1, 100, i32::MAX] {
        assert_json_codec::<checkpoints_v1::ScheduleCheckpoint>(json!({"status": number, "lastOutcome": number}));
        assert_json_codec::<state_v1::State>(json!({"state": number}));
    }
}

#[test]
fn invalid_storage_values_are_not_silently_coerced() {
    for input in [
        r#"{"status":"SCHEDULE_CHECKPOINT_STATUS_FUTURE"}"#,
        r#"{"lastOutcome":"RECONCILE_OUTCOME_FUTURE"}"#,
        r#"{"lastAppliedStreamPosition":"18446744073709551616"}"#,
        r#"{"lastAppliedStreamPosition":-1}"#,
        r#"{"lastAppliedStreamPosition":1.5}"#,
        r#"{"lastAppliedStreamPosition":"1","last_applied_stream_position":"2"}"#,
        r#"{"status":2147483648}"#,
    ] {
        assert_rejected::<checkpoints_v1::ScheduleCheckpoint>(input);
    }
    for input in [
        r#"{"state":"STATE_VALUE_FUTURE"}"#,
        r#"{"state":true}"#,
        r#"{"completed":"false"}"#,
        r#"{"lastOccurrenceSequence":"-1"}"#,
        r#"{"lastOccurrenceAt":"not-a-timestamp"}"#,
        r#"{"lastOccurrenceSequence":"1","last_occurrence_sequence":"2"}"#,
    ] {
        assert_rejected::<state_v1::State>(input);
    }
}

macro_rules! status_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn status_json_distinguishes_absence_from_each_selected_variant() {
                assert_normalized::<schema::ScheduleStatus>(json!({"scheduled": null, "paused": null, "future": {"v": 1}}), json!({}));
                for input in ["true", "[]", r#"{"scheduled":1}"#, r#"{"paused":"yes"}"#,
                    r#"{"scheduled":{},"paused":{}}"#, r#"{"scheduled":{},"scheduled":{}}"#, r#"{"paused":{},"paused":{}}"#] {
                    assert_rejected::<schema::ScheduleStatus>(input);
                }
                assert_normalized::<schema::ScheduleStatus>(json!({"scheduled": null, "paused": {}}), json!({"paused": {}}));
            }

            #[test]
            fn status_wire_merges_same_variant_and_last_different_variant_wins() {
                let scheduled = assert_json_codec::<schema::ScheduleStatus>(json!({"scheduled": {}}));
                let paused = assert_json_codec::<schema::ScheduleStatus>(json!({"paused": {}}));
                assert_wire_codec(b"\x0a\x00\x0a\x03\xf8\x07\x01", &scheduled);
                assert_wire_codec(b"\x12\x00\x12\x03\xf8\x07\x01", &paused);
                assert_wire_codec(b"\x12\x00\x0a\x00", &scheduled);
                assert_wire_codec(b"\x0a\x00\x12\x00", &paused);
                assert_length_delimited_fields::<schema::ScheduleStatus>(&[1, 2]);
            }
        }
    };
}

status_contracts!(live_status_tests, v1);
status_contracts!(projection_status_tests, projections_v1);

#[test]
fn lifecycle_event_json_validates_all_payloads_and_rejects_conflicting_variants() {
    let fields = [
        "scheduleCreated",
        "schedulePaused",
        "scheduleResumed",
        "scheduleRemoved",
        "scheduleOccurrenceScheduled",
        "scheduleOccurrenceRecorded",
        "scheduleCompleted",
    ];
    for field in fields {
        assert_normalized::<v1::ScheduleEvent>(json!({field: null, "futureEvent": {"nested": []}}), json!({}));
        assert!(serde_json::from_value::<v1::ScheduleEvent>(json!({field: true})).is_err());
        let duplicate = format!("{{\"{field}\":{{}},\"{field}\":{{}}}}");
        assert_rejected::<v1::ScheduleEvent>(&duplicate);
    }
    assert_rejected::<v1::ScheduleEvent>("true");
    assert_rejected::<v1::ScheduleEvent>(r#"{"schedulePaused":{},"scheduleResumed":{}}"#);
    assert_length_delimited_fields::<v1::ScheduleEvent>(&[1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn checkpoint_builders_keep_explicit_default_fields_on_the_wire() {
    let checkpoint = checkpoints_v1::ScheduleCheckpoint::default()
        .with_schedule_id("")
        .with_status(checkpoints_v1::ScheduleCheckpointStatus::Unspecified)
        .with_last_applied_stream_position(0)
        .with_last_applied_event_id("")
        .with_last_outcome(checkpoints_v1::ReconcileOutcome::Unspecified);
    let expected = assert_json_codec::<checkpoints_v1::ScheduleCheckpoint>(json!({
        "scheduleId": "", "status": "SCHEDULE_CHECKPOINT_STATUS_UNSPECIFIED",
        "lastAppliedStreamPosition": "0", "lastAppliedEventId": "", "lastOutcome": "RECONCILE_OUTCOME_UNSPECIFIED"
    }));
    assert_eq!(checkpoint, expected);
    assert_ne!(checkpoint, checkpoints_v1::ScheduleCheckpoint::default());
    assert_wire_codec(b"\x0a\x00\x10\x00\x18\x00\x22\x00\x28\x00", &checkpoint);
    let state = state_v1::State::default()
        .with_state(state_v1::StateValue::Unspecified)
        .with_last_occurrence_sequence(0)
        .with_completed(false);
    assert_eq!(
        state,
        assert_json_codec::<state_v1::State>(json!({
            "state": "STATE_VALUE_UNSPECIFIED", "lastOccurrenceSequence": "0", "completed": false
        }))
    );
    assert_ne!(state, state_v1::State::default());
    assert_wire_codec(b"\x08\x00\x18\x00\x30\x00", &state);
}

#[test]
fn repeated_event_variants_merge_without_clearing_earlier_fields() {
    let created = json!({"scheduleCreated": {
        "scheduleId": "job", "status": {"scheduled": {}}, "schedule": {"every": {"every": "5s"}},
        "delivery": {}, "message": {"content": {}}
    }});
    let first: v1::ScheduleEvent = serde_json::from_value(created).expect("first creation fragment");
    let expected: v1::ScheduleEvent = serde_json::from_value(json!({"scheduleCreated": {
        "scheduleId": "job", "status": {"paused": {}}, "schedule": {"every": {"every": "5s"}},
        "delivery": {}, "message": {"content": {}}
    }}))
    .expect("creation with merged status");
    assert_wire_codec(
        &[first.encode_to_vec(), b"\x0a\x04\x12\x02\x12\x00".to_vec()].concat(),
        &expected,
    );

    for (wire, expected) in [
        (
            b"\x12\x05\x0a\x03job\x12\x03\xf8\x07\x01".as_slice(),
            json!({"schedulePaused": {"scheduleId": "job"}}),
        ),
        (
            b"\x1a\x05\x0a\x03job\x1a\x03\xf8\x07\x01".as_slice(),
            json!({"scheduleResumed": {"scheduleId": "job"}}),
        ),
        (
            b"\x22\x05\x0a\x03job\x22\x03\xf8\x07\x01".as_slice(),
            json!({"scheduleRemoved": {"scheduleId": "job"}}),
        ),
        (
            b"\x2a\x0b\x0a\x03job\x10\x07\x1a\x02\x08\x01\x2a\x04\x22\x02\x08\x03".as_slice(),
            json!({"scheduleOccurrenceRecorded": {
                "scheduleId": "job", "occurrenceSequence": "7", "occurrenceAt": "1970-01-01T00:00:01Z", "recordedAt": "1970-01-01T00:00:03Z"
            }}),
        ),
        (
            b"\x32\x0b\x0a\x03job\x10\x07\x1a\x02\x08\x01\x32\x04\x22\x02\x08\x02".as_slice(),
            json!({"scheduleOccurrenceScheduled": {
                "scheduleId": "job", "occurrenceSequence": "7", "occurrenceAt": "1970-01-01T00:00:01Z", "scheduledAt": "1970-01-01T00:00:02Z"
            }}),
        ),
        (
            b"\x3a\x05\x0a\x03job\x3a\x02\x10\x07".as_slice(),
            json!({"scheduleCompleted": {"scheduleId": "job", "lastOccurrenceSequence": "7"}}),
        ),
    ] {
        let expected: v1::ScheduleEvent = serde_json::from_value(expected).expect("merged lifecycle event");
        assert_wire_codec(wire, &expected);
    }
}
