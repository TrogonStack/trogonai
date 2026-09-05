use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, OwnedView};
use serde_json::{Value, json};

use super::assert_json_codec;
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};

macro_rules! assert_retained {
    ($owned:ty, $retained:ty, $fixture:expr, |$handle:ident| $checks:block) => {{
        let fixture = $fixture;
        let expected = assert_json_codec::<$owned>(fixture.clone());
        let retained = {
            let mut source = expected.clone();
            let retained = <$retained>::from_owned(&source).expect("retain owned message");
            source.clear();
            retained
        };
        let independent = retained.clone();
        drop(retained);
        let $handle = std::thread::spawn(move || independent)
            .join()
            .expect("retained message can cross a thread boundary");
        $checks
        assert_eq!(serde_json::to_value($handle.view()).expect("retained view JSON"), fixture);
        assert_eq!($handle.to_owned_message(), expected);
        let mut wire = $handle.into_bytes().to_vec();
        wire.extend_from_slice(&[0xf8, 0x07, 0x01]);
        let too_small = DecodeOptions::new().with_max_message_size(wire.len() - 1);
        assert_eq!(
            <$retained>::decode_with_options(Bytes::copy_from_slice(&wire), &too_small).err(),
            Some(DecodeError::MessageTooLarge)
        );
        let options = DecodeOptions::new().with_max_message_size(wire.len());
        let retained = <$retained>::decode_with_options(Bytes::copy_from_slice(&wire), &options)
            .expect("bounded retained message");
        assert_eq!(retained.bytes().as_ref(), wire.as_slice());
        let pointer = retained.bytes().as_ptr();
        let raw: OwnedView<<$owned as HasMessageView>::View<'static>> = retained.into();
        let retained = <$retained>::from(raw);
        let transferred = retained.into_bytes();
        assert_eq!(transferred.as_ptr(), pointer, "ownership transfer must retain the allocation");
        assert_eq!(transferred.as_ref(), wire.as_slice(), "unknown wire fields must survive transfer");
        let decoded = <$retained>::decode(transferred).expect("transferred retained message");
        assert_eq!(decoded.to_owned_message(), expected);
    }};
}

fn recurrence() -> Value {
    json!({"dtstart": "2026-01-01T00:00:00.123Z", "rrule": "FREQ=DAILY;COUNT=5",
        "timezone": {"id": "UTC", "version": "2025b"},
        "rdate": ["2026-01-07T00:00:00Z", "2026-01-08T00:00:00Z"],
        "exdate": ["2026-01-02T00:00:00Z"]})
}

fn schedule() -> Value {
    json!({"rrule": recurrence()})
}

fn nats_message() -> Value {
    json!({"subject": "jobs.run", "ttl": "30s",
        "source": {"latestFromSubject": {"subject": "jobs.template"}}})
}

fn delivery() -> Value {
    json!({"natsMessage": nats_message()})
}

fn message() -> Value {
    json!({"content": {"contentType": "application/octet-stream", "data": "AP8B"},
        "headers": [{"name": "x-priority", "value": "urgent"}, {"name": "x-priority", "value": "normal"}]})
}

macro_rules! shared_retention_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn schedule_definitions_survive_source_drop_and_buffer_transfer() {
                assert_retained!(schema::Schedule, schema::ScheduleOwnedView, schedule(), |handle| {
                    assert!(matches!(handle.kind(), Some(schema::schedule::KindView::Rrule(_))));
                });
                assert_retained!(schema::schedule::At, schema::schedule::AtOwnedView,
                    json!({"at": "2026-01-01T00:00:00.123Z"}), |handle| {
                        assert_eq!(serde_json::to_value(handle.at().as_option().expect("present at")).expect("at timestamp"), json!("2026-01-01T00:00:00.123Z"));
                    });
                assert_retained!(schema::schedule::Every, schema::schedule::EveryOwnedView,
                    json!({"every": "60.500s"}), |handle| {
                        assert_eq!(serde_json::to_value(handle.every().as_option().expect("present every")).expect("interval"), json!("60.500s"));
                    });
                assert_retained!(schema::schedule::Cron, schema::schedule::CronOwnedView,
                    json!({"expr": "0 9 * * *", "timezone": {"id": "Europe/Paris", "version": "2025b"}}), |handle| {
                        assert_eq!(handle.expr(), "0 9 * * *");
                        assert_eq!(serde_json::to_value(handle.timezone().as_option().expect("present timezone")).expect("timezone"), json!({"id": "Europe/Paris", "version": "2025b"}));
                    });
                assert_retained!(schema::schedule::RRule, schema::schedule::RRuleOwnedView, recurrence(), |handle| {
                    assert_eq!(handle.rrule(), "FREQ=DAILY;COUNT=5");
                    assert_eq!(serde_json::to_value(handle.dtstart().as_option().expect("present dtstart")).expect("start"), json!("2026-01-01T00:00:00.123Z"));
                    assert_eq!(serde_json::to_value(handle.timezone().as_option().expect("present timezone")).expect("timezone"), json!({"id": "UTC", "version": "2025b"}));
                    assert_eq!(serde_json::to_value(&**handle.rdate()).expect("extra dates"), json!(["2026-01-07T00:00:00Z", "2026-01-08T00:00:00Z"]));
                    assert_eq!(serde_json::to_value(&**handle.exdate()).expect("excluded dates"), json!(["2026-01-02T00:00:00Z"]));
                });
            }

            #[test]
            fn delivery_sources_survive_source_drop_and_buffer_transfer() {
                assert_retained!(schema::Delivery, schema::DeliveryOwnedView, delivery(), |handle| {
                    assert!(matches!(handle.kind(), Some(schema::delivery::KindView::NatsMessage(_))));
                });
                assert_retained!(schema::delivery::NatsMessage, schema::delivery::NatsMessageOwnedView,
                    nats_message(), |handle| {
                        assert_eq!(handle.subject(), "jobs.run");
                        assert_eq!(serde_json::to_value(handle.ttl().as_option().expect("present ttl")).expect("ttl"), json!("30s"));
                        assert_eq!(serde_json::to_value(handle.source().as_option().expect("present source")).expect("source"), json!({"latestFromSubject": {"subject": "jobs.template"}}));
                    });
                assert_retained!(schema::delivery::nats_message::Source, schema::delivery::nats_message::SourceOwnedView,
                    json!({"latestFromSubject": {"subject": "jobs.template"}}), |handle| {
                        assert!(matches!(handle.kind(), Some(schema::delivery::nats_message::source::KindView::LatestFromSubject(_))));
                    });
                assert_retained!(schema::delivery::nats_message::LatestFromSubject, schema::delivery::nats_message::LatestFromSubjectOwnedView,
                    json!({"subject": "jobs.template"}), |handle| {
                        assert_eq!(handle.subject(), "jobs.template");
                    });
            }

            #[test]
            fn message_headers_preserve_order_and_binary_payload_after_transfer() {
                assert_retained!(schema::Message, schema::MessageOwnedView, message(), |handle| {
                    assert_eq!(serde_json::to_value(handle.content().as_option().expect("present content")).expect("content"), json!({"contentType": "application/octet-stream", "data": "AP8B"}));
                    assert_eq!(serde_json::to_value(&**handle.headers()).expect("headers"), json!([
                        {"name": "x-priority", "value": "urgent"}, {"name": "x-priority", "value": "normal"}
                    ]));
                });
                assert_retained!(schema::Header, schema::HeaderOwnedView,
                    json!({"name": "x-priority", "value": "urgent"}), |handle| {
                        assert_eq!(handle.name(), "x-priority");
                        assert_eq!(handle.value(), "urgent");
                    });
            }
        }
    };
}

shared_retention_contracts!(live_tests, v1);
shared_retention_contracts!(checkpoint_tests, checkpoints_v1);
shared_retention_contracts!(projection_tests, projections_v1);

#[test]
fn checkpoint_retention_preserves_replay_fence_and_definition() {
    assert_retained!(
        checkpoints_v1::ScheduleCheckpoint,
        checkpoints_v1::ScheduleCheckpointOwnedView,
        json!({"scheduleId": "job", "status": "SCHEDULE_CHECKPOINT_STATUS_PAUSED",
            "lastAppliedStreamPosition": "9007199254740993", "lastAppliedEventId": "event-7",
            "lastOutcome": "RECONCILE_OUTCOME_STORED_PAUSED", "schedule": schedule(),
            "delivery": delivery(), "message": message()}),
        |handle| {
            assert_eq!(handle.schedule_id(), Some("job"));
            assert_eq!(handle.last_applied_stream_position(), Some(9_007_199_254_740_993));
            assert_eq!(handle.last_applied_event_id(), Some("event-7"));
            assert_eq!(
                handle.status(),
                Some(checkpoints_v1::ScheduleCheckpointStatus::Paused.into())
            );
            assert_eq!(
                handle.last_outcome(),
                Some(checkpoints_v1::ReconcileOutcome::StoredPaused.into())
            );
            assert_eq!(
                serde_json::to_value(handle.schedule().as_option().expect("present schedule")).expect("schedule"),
                schedule()
            );
            assert_eq!(
                serde_json::to_value(handle.delivery().as_option().expect("present delivery")).expect("delivery"),
                delivery()
            );
            assert_eq!(
                serde_json::to_value(handle.message().as_option().expect("present message")).expect("message"),
                message()
            );
        }
    );
}

#[test]
fn projection_retention_preserves_completion_and_occurrence_timestamps() {
    assert_retained!(
        projections_v1::ScheduleProjection,
        projections_v1::ScheduleProjectionOwnedView,
        json!({"scheduleId": "job", "status": {"paused": {}}, "completed": false,
            "nextOccurrenceAt": "2026-01-02T00:00:00Z", "lastOccurrenceAt": "2026-01-01T00:00:00Z",
            "schedule": schedule(), "delivery": delivery(), "message": message()}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(handle.completed(), Some(false));
            assert_eq!(
                serde_json::to_value(handle.status().as_option().expect("present status")).expect("status"),
                json!({"paused": {}})
            );
            assert_eq!(
                serde_json::to_value(
                    handle
                        .next_occurrence_at()
                        .as_option()
                        .expect("present next_occurrence_at")
                )
                .expect("next"),
                json!("2026-01-02T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(
                    handle
                        .last_occurrence_at()
                        .as_option()
                        .expect("present last_occurrence_at")
                )
                .expect("last"),
                json!("2026-01-01T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(handle.schedule().as_option().expect("present schedule")).expect("schedule"),
                schedule()
            );
            assert_eq!(
                serde_json::to_value(handle.delivery().as_option().expect("present delivery")).expect("delivery"),
                delivery()
            );
            assert_eq!(
                serde_json::to_value(handle.message().as_option().expect("present message")).expect("message"),
                message()
            );
        }
    );
}

#[test]
fn state_retention_preserves_pending_occurrence_and_present_zero() {
    assert_retained!(
        state_v1::State,
        state_v1::StateOwnedView,
        json!({"state": "STATE_VALUE_PRESENT_ENABLED", "lastOccurrenceAt": "2026-01-01T00:00:00Z",
            "lastOccurrenceSequence": "0", "schedule": schedule(), "pendingOccurrenceAt": "2026-01-02T00:00:00Z",
            "completed": false}),
        |handle| {
            assert_eq!(handle.state(), Some(state_v1::StateValue::PresentEnabled.into()));
            assert_eq!(handle.last_occurrence_sequence(), Some(0));
            assert_eq!(handle.completed(), Some(false));
            assert_eq!(
                serde_json::to_value(
                    handle
                        .last_occurrence_at()
                        .as_option()
                        .expect("present last_occurrence_at")
                )
                .expect("last"),
                json!("2026-01-01T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(
                    handle
                        .pending_occurrence_at()
                        .as_option()
                        .expect("present pending_occurrence_at")
                )
                .expect("pending"),
                json!("2026-01-02T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(handle.schedule().as_option().expect("present schedule")).expect("schedule"),
                schedule()
            );
        }
    );
}

#[test]
fn lifecycle_status_retention_keeps_the_selected_empty_message() {
    assert_retained!(
        v1::ScheduleStatus,
        v1::ScheduleStatusOwnedView,
        json!({"scheduled": {}}),
        |handle| {
            assert!(matches!(
                handle.kind(),
                Some(v1::schedule_status::KindView::Scheduled(_))
            ));
        }
    );
    assert_retained!(
        projections_v1::ScheduleStatus,
        projections_v1::ScheduleStatusOwnedView,
        json!({"paused": {}}),
        |handle| {
            assert!(matches!(
                handle.kind(),
                Some(projections_v1::schedule_status::KindView::Paused(_))
            ));
        }
    );
}

#[test]
fn create_command_retention_preserves_the_requested_definition() {
    assert_retained!(
        v1::CreateSchedule,
        v1::CreateScheduleOwnedView,
        json!({"scheduleId": "job", "status": {"scheduled": {}}, "schedule": schedule(), "delivery": delivery(), "message": message()}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(
                serde_json::to_value(handle.status().as_option().expect("present status")).expect("status"),
                json!({"scheduled": {}})
            );
            assert_eq!(
                serde_json::to_value(handle.schedule().as_option().expect("present schedule")).expect("schedule"),
                schedule()
            );
            assert_eq!(
                serde_json::to_value(handle.delivery().as_option().expect("present delivery")).expect("delivery"),
                delivery()
            );
            assert_eq!(
                serde_json::to_value(handle.message().as_option().expect("present message")).expect("message"),
                message()
            );
        }
    );
}

#[test]
fn created_event_retention_keeps_the_original_command_definition() {
    assert_retained!(
        v1::ScheduleCreated,
        v1::ScheduleCreatedOwnedView,
        json!({"scheduleId": "job", "status": {"scheduled": {}}, "schedule": schedule(), "delivery": delivery(), "message": message()}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(
                serde_json::to_value(handle.status().as_option().expect("status")).expect("status JSON"),
                json!({"scheduled": {}})
            );
            assert_eq!(
                serde_json::to_value(handle.schedule().as_option().expect("schedule")).expect("schedule JSON"),
                schedule()
            );
            assert_eq!(
                serde_json::to_value(handle.delivery().as_option().expect("delivery")).expect("delivery JSON"),
                delivery()
            );
            assert_eq!(
                serde_json::to_value(handle.message().as_option().expect("message")).expect("message JSON"),
                message()
            );
        }
    );
}

#[test]
fn lifecycle_commands_and_events_keep_schedule_identity_after_transfer() {
    assert_retained!(
        v1::PauseSchedule,
        v1::PauseScheduleOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::ResumeSchedule,
        v1::ResumeScheduleOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::RemoveSchedule,
        v1::RemoveScheduleOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::SchedulePaused,
        v1::SchedulePausedOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::ScheduleResumed,
        v1::ScheduleResumedOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::ScheduleRemoved,
        v1::ScheduleRemovedOwnedView,
        json!({"scheduleId": "job"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
        }
    );
    assert_retained!(
        v1::ScheduleCompleted,
        v1::ScheduleCompletedOwnedView,
        json!({"scheduleId": "job", "lastOccurrenceSequence": "18446744073709551615"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(handle.last_occurrence_sequence(), Some(u64::MAX));
        }
    );
}

#[test]
fn retained_occurrences_distinguish_scheduled_and_recorded_wall_clock_times() {
    assert_retained!(
        v1::ScheduleOccurrenceScheduled,
        v1::ScheduleOccurrenceScheduledOwnedView,
        json!({"scheduleId": "job", "occurrenceSequence": "9007199254740993",
            "occurrenceAt": "2026-01-02T00:00:00Z", "scheduledAt": "2026-01-01T23:59:59Z"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(handle.occurrence_sequence(), Some(9_007_199_254_740_993));
            assert_eq!(
                serde_json::to_value(handle.occurrence_at().as_option().expect("occurrence")).expect("occurrence JSON"),
                json!("2026-01-02T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(handle.scheduled_at().as_option().expect("scheduled")).expect("scheduled JSON"),
                json!("2026-01-01T23:59:59Z")
            );
        }
    );
    assert_retained!(
        v1::ScheduleOccurrenceRecorded,
        v1::ScheduleOccurrenceRecordedOwnedView,
        json!({"scheduleId": "job", "occurrenceSequence": "9007199254740993",
            "occurrenceAt": "2026-01-02T00:00:00Z", "recordedAt": "2026-01-02T00:00:03Z"}),
        |handle| {
            assert_eq!(handle.schedule_id(), "job");
            assert_eq!(handle.occurrence_sequence(), Some(9_007_199_254_740_993));
            assert_eq!(
                serde_json::to_value(handle.occurrence_at().as_option().expect("occurrence")).expect("occurrence JSON"),
                json!("2026-01-02T00:00:00Z")
            );
            assert_eq!(
                serde_json::to_value(handle.recorded_at().as_option().expect("recorded")).expect("recorded JSON"),
                json!("2026-01-02T00:00:03Z")
            );
        }
    );
}

#[test]
fn event_envelope_retention_preserves_each_lifecycle_payload() {
    for expected in [
        json!({"scheduleCreated": {"scheduleId": "job", "status": {"scheduled": {}}, "schedule": schedule(), "delivery": delivery(), "message": message()}}),
        json!({"schedulePaused": {"scheduleId": "job"}}),
        json!({"scheduleResumed": {"scheduleId": "job"}}),
        json!({"scheduleRemoved": {"scheduleId": "job"}}),
        json!({"scheduleOccurrenceScheduled": {"scheduleId": "job", "occurrenceSequence": "7", "occurrenceAt": "2026-01-02T00:00:00Z", "scheduledAt": "2026-01-01T23:59:59Z"}}),
        json!({"scheduleOccurrenceRecorded": {"scheduleId": "job", "occurrenceSequence": "7", "occurrenceAt": "2026-01-02T00:00:00Z", "recordedAt": "2026-01-02T00:00:03Z"}}),
        json!({"scheduleCompleted": {"scheduleId": "job", "lastOccurrenceSequence": "7"}}),
    ] {
        assert_retained!(v1::ScheduleEvent, v1::ScheduleEventOwnedView, expected, |handle| {
            let schedule_id = match handle.event().expect("selected event") {
                v1::schedule_event::EventView::ScheduleCreated(event) => event.schedule_id,
                v1::schedule_event::EventView::SchedulePaused(event) => event.schedule_id,
                v1::schedule_event::EventView::ScheduleResumed(event) => event.schedule_id,
                v1::schedule_event::EventView::ScheduleRemoved(event) => event.schedule_id,
                v1::schedule_event::EventView::ScheduleOccurrenceScheduled(event) => event.schedule_id,
                v1::schedule_event::EventView::ScheduleOccurrenceRecorded(event) => event.schedule_id,
                v1::schedule_event::EventView::ScheduleCompleted(event) => event.schedule_id,
            };
            assert_eq!(schedule_id, "job");
        });
    }
}

macro_rules! retained_status_variants {
    ($schema:ident) => {{
        assert_retained!(
            $schema::schedule_status::Scheduled,
            $schema::schedule_status::ScheduledOwnedView,
            json!({}),
            |handle| {
                assert!(handle.bytes().is_empty());
            }
        );
        assert_retained!(
            $schema::schedule_status::Paused,
            $schema::schedule_status::PausedOwnedView,
            json!({}),
            |handle| {
                assert!(handle.bytes().is_empty());
            }
        );
    }};
}

#[test]
fn status_variant_handles_preserve_unknown_fields_when_forwarded() {
    retained_status_variants!(v1);
    retained_status_variants!(projections_v1);
}
