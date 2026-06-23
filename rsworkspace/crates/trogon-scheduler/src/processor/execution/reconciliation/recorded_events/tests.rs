use buffa::MessageField;

use super::*;
use crate::commands::domain::MessageEnvelope;
use crate::commands::domain::{
    Delivery as DomainDelivery, MessageContent as DomainContent, RRuleTimezone, SamplingSource as DomainSource,
    Schedule as DomainSchedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleHeaders as DomainHeaders,
    ScheduleMessage as DomainMessage,
};

fn at_instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2027-03-04T05:06:07Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn proto_schedule(schedule: &DomainSchedule) -> v1::Schedule {
    v1::Schedule::try_from(&ScheduleEventSchedule::from(schedule)).unwrap()
}

fn proto_delivery(delivery: &DomainDelivery) -> v1::Delivery {
    v1::Delivery::try_from(&ScheduleEventDelivery::from(delivery)).unwrap()
}

fn proto_message(message: &DomainMessage) -> v1::Message {
    v1::Message::from(&MessageEnvelope::from(message))
}

#[test]
fn schedule_round_trips_through_proto_for_every_kind() {
    let schedules = [
        DomainSchedule::At { at: at_instant() },
        DomainSchedule::every(Duration::from_secs(90)).unwrap(),
        DomainSchedule::cron("0 0 * * * *", Some("America/New_York".to_string())).unwrap(),
        DomainSchedule::cron("0 0 * * * *", None).unwrap(),
    ];

    for schedule in schedules {
        let decoded = schedule_from_proto(&proto_schedule(&schedule)).unwrap();
        assert_eq!(decoded, schedule);
    }
}

#[test]
fn rrule_round_trips_through_proto() {
    let rrule = DomainSchedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", Some("UTC".to_string())).unwrap();
    let decoded = schedule_from_proto(&proto_schedule(&rrule)).unwrap();

    let DomainSchedule::RRule {
        rrule: a,
        timezone: tz_a,
        ..
    } = decoded
    else {
        panic!("expected decoded RRule schedule");
    };
    let DomainSchedule::RRule {
        rrule: b,
        timezone: tz_b,
        ..
    } = rrule
    else {
        panic!("expected source RRule schedule");
    };
    assert_eq!(a.as_str(), b.as_str());
    assert_eq!(
        tz_a.map(|tz| tz.as_str().to_string()),
        tz_b.map(|tz| tz.as_str().to_string())
    );
    let _ = RRuleTimezone::new("UTC").unwrap();
}

#[test]
fn delivery_round_trips_with_ttl_and_source() {
    let delivery = DomainDelivery::NatsEvent {
        route: DeliveryRoute::new("agent.run").unwrap(),
        ttl: Some(TtlDuration::from_secs(60).unwrap()),
        source: Some(DomainSource::latest_from_subject("agent.events").unwrap()),
    };
    let decoded = delivery_from_proto(&proto_delivery(&delivery)).unwrap();
    assert_eq!(decoded, delivery);
}

#[test]
fn message_round_trips_content_type_and_headers() {
    let message = DomainMessage {
        content: DomainContent::json(r#"{"ok":true}"#),
        headers: DomainHeaders::new([("x-kind", "heartbeat")]).unwrap(),
    };
    let decoded = message_from_proto(&proto_message(&message)).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn created_event_missing_schedule_field_is_rejected() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
        schedule: MessageField::none(),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };
    let event = v1::ScheduleEvent {
        event: Some(created.into()),
    };

    let error = decode_schedule_change(&event).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "schedule" }
    ));
}

#[test]
fn schedule_at_missing_timestamp_is_rejected() {
    use trogonai_proto::scheduler::schedules::v1::schedule;
    let schedule = v1::Schedule {
        kind: Some(v1::schedule::Kind::At(Box::new(schedule::At {
            at: MessageField::none(),
        }))),
    };
    let error = schedule_from_proto(&schedule).unwrap_err();
    assert!(matches!(error, ScheduleEventDecodeError::MissingField { field: "at" }));
}

#[test]
fn message_without_content_defaults_payload() {
    let message = v1::Message {
        content: MessageField::none(),
        headers: Vec::new(),
    };
    let decoded = message_from_proto(&message).unwrap();
    assert_eq!(decoded.content, DomainContent::default());
}

#[test]
fn created_event_missing_delivery_field_is_rejected() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::none(),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };
    let event = v1::ScheduleEvent {
        event: Some(created.into()),
    };

    let error = decode_schedule_change(&event).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "delivery" }
    ));
}

#[test]
fn created_event_missing_message_field_is_rejected() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::none(),
    };
    let event = v1::ScheduleEvent {
        event: Some(created.into()),
    };

    let error = decode_schedule_change(&event).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "message" }
    ));
}

#[test]
fn schedule_every_missing_duration_is_rejected() {
    use trogonai_proto::scheduler::schedules::v1::schedule;
    let schedule = v1::Schedule {
        kind: Some(v1::schedule::Kind::Every(Box::new(schedule::Every {
            every: MessageField::none(),
        }))),
    };
    let error = schedule_from_proto(&schedule).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "every" }
    ));
}

#[test]
fn schedule_rrule_missing_dtstart_is_rejected() {
    use trogonai_proto::scheduler::schedules::v1::schedule;
    let schedule = v1::Schedule {
        kind: Some(v1::schedule::Kind::Rrule(Box::new(schedule::RRule {
            dtstart: MessageField::none(),
            rrule: "FREQ=DAILY;COUNT=1".to_string(),
            timezone: MessageField::none(),
            rdate: Vec::new(),
            exdate: Vec::new(),
        }))),
    };
    let error = schedule_from_proto(&schedule).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "dtstart" }
    ));
}

#[test]
fn timezone_with_version_round_trips() {
    let timezone = trogonai_proto::google::r#type::TimeZone {
        id: "America/New_York".to_string(),
        version: "2025b".to_string(),
    };
    let decoded = timezone_from_proto(Some(&timezone)).unwrap().expect("timezone");
    assert_eq!(decoded.as_str(), "America/New_York");
}

#[test]
fn schedule_proto_kinds_decode_successfully() {
    let at = schedule_from_proto(&proto_schedule(&DomainSchedule::At { at: at_instant() })).unwrap();
    assert!(matches!(at, DomainSchedule::At { .. }));

    let every = schedule_from_proto(&proto_schedule(&DomainSchedule::every(Duration::from_secs(5)).unwrap())).unwrap();
    assert!(matches!(every, DomainSchedule::Every { .. }));

    let cron = schedule_from_proto(&proto_schedule(&DomainSchedule::cron("0 0 * * * *", None).unwrap())).unwrap();
    assert!(matches!(cron, DomainSchedule::Cron { .. }));

    let rrule = schedule_from_proto(&proto_schedule(
        &DomainSchedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=1", Some("UTC".to_string())).unwrap(),
    ))
    .unwrap();
    assert!(matches!(rrule, DomainSchedule::RRule { .. }));
}

#[test]
fn definition_from_created_rejects_missing_schedule() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::none(),
        schedule: MessageField::none(),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };

    let error = definition_from_created(&created).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "schedule" }
    ));
}

#[test]
fn definition_from_created_rejects_missing_delivery() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::none(),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::none(),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };

    let error = definition_from_created(&created).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "delivery" }
    ));
}

#[test]
fn definition_from_created_rejects_missing_message() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::none(),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::none(),
    };

    let error = definition_from_created(&created).unwrap_err();
    assert!(matches!(
        error,
        ScheduleEventDecodeError::MissingField { field: "message" }
    ));
}

#[test]
fn definition_from_created_decodes_all_fields() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };

    let definition = definition_from_created(&created).unwrap();
    assert_eq!(definition.status, ScheduleEventStatus::Paused);
}

#[test]
fn created_event_decodes_into_a_schedule_change() {
    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };
    let event = v1::ScheduleEvent {
        event: Some(created.into()),
    };

    let ScheduleChange::Created {
        schedule_id,
        definition,
    } = decode_schedule_change(&event).unwrap()
    else {
        panic!("expected Created");
    };
    assert_eq!(schedule_id.as_str(), "orders/created");
    assert_eq!(definition.status, ScheduleEventStatus::Paused);
}

#[test]
fn recorded_events_decode_for_pause_resume_remove() {
    for (event, expect_id) in [
        (
            v1::ScheduleEvent {
                event: Some(
                    v1::SchedulePaused {
                        schedule_id: "a".to_string(),
                    }
                    .into(),
                ),
            },
            "a",
        ),
        (
            v1::ScheduleEvent {
                event: Some(
                    v1::ScheduleResumed {
                        schedule_id: "b".to_string(),
                    }
                    .into(),
                ),
            },
            "b",
        ),
        (
            v1::ScheduleEvent {
                event: Some(
                    v1::ScheduleRemoved {
                        schedule_id: "c".to_string(),
                    }
                    .into(),
                ),
            },
            "c",
        ),
    ] {
        let change = decode_schedule_change(&event).unwrap();
        assert_eq!(change.schedule_id().as_str(), expect_id);
    }
}

#[test]
fn missing_event_case_is_an_error() {
    let event = v1::ScheduleEvent { event: None };
    assert!(matches!(
        decode_schedule_change(&event).unwrap_err(),
        ScheduleEventDecodeError::MissingEvent
    ));
}

#[test]
fn occurrence_lifecycle_events_decode_into_schedule_changes() {
    use trogon_decider_runtime::{Event, EventEncode, EventId, EventType, Headers, StreamEvent, StreamPosition};
    use uuid::Uuid;

    let occurrence_at = at_instant();
    let event = v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceRecorded {
                schedule_id: "backup".to_string(),
                occurrence_sequence: Some(2),
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
                recorded_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
            }
            .into(),
        ),
    };

    let change = decode_schedule_change(&event).unwrap();
    assert!(matches!(
        change,
        ScheduleChange::OccurrenceRecorded {
            ref schedule_id,
            ref occurrence_sequence,
            occurrence_at: decoded_at,
        } if schedule_id.as_str() == "backup"
            && occurrence_sequence.as_u64() == 2
            && decoded_at == occurrence_at
    ));

    let stream_event = StreamEvent {
        stream_id: "backup".to_string(),
        event: Event {
            id: EventId::new(Uuid::from_u128(3)),
            r#type: event.event_type().expect("event has a type").to_string(),
            content: EventEncode::encode(&event).expect("event encodes"),
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
        recorded_at: at_instant(),
    };

    assert!(matches!(
        schedule_change_from_stream_event(&stream_event).unwrap(),
        Some(ScheduleChange::OccurrenceRecorded { .. })
    ));

    let event = v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceScheduled {
                schedule_id: "backup".to_string(),
                occurrence_sequence: Some(3),
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
                scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
            }
            .into(),
        ),
    };
    let change = decode_schedule_change(&event).unwrap();
    assert!(matches!(
        change,
        ScheduleChange::OccurrenceScheduled {
            ref schedule_id,
            ref occurrence_sequence,
            occurrence_at: decoded_at,
        } if schedule_id.as_str() == "backup"
            && occurrence_sequence.as_u64() == 3
            && decoded_at == occurrence_at
    ));
}

#[test]
fn scheduled_occurrence_requires_valid_sequence() {
    let event = v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceScheduled {
                schedule_id: "backup".to_string(),
                occurrence_sequence: None,
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at_instant())),
                scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at_instant())),
            }
            .into(),
        ),
    };
    assert!(matches!(
        decode_schedule_change(&event).unwrap_err(),
        ScheduleEventDecodeError::MissingField {
            field: "occurrence_sequence",
        }
    ));

    let event = v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceScheduled {
                schedule_id: "backup".to_string(),
                occurrence_sequence: Some(0),
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at_instant())),
                scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at_instant())),
            }
            .into(),
        ),
    };
    assert!(matches!(
        decode_schedule_change(&event).unwrap_err(),
        ScheduleEventDecodeError::OccurrenceSequence { .. }
    ));
}

#[test]
fn negative_duration_is_rejected() {
    let error = proto_duration_to_std(&ProtoDuration::from_secs_nanos(-1, 0), "every").unwrap_err();
    assert!(matches!(error, ScheduleEventDecodeError::Duration { field: "every" }));
}

#[test]
fn out_of_range_duration_nanos_is_rejected() {
    let mut duration = ProtoDuration::from_secs_nanos(1, 0);
    duration.nanos = 1_000_000_000;
    let error = proto_duration_to_std(&duration, "every").unwrap_err();
    assert!(matches!(error, ScheduleEventDecodeError::Duration { field: "every" }));
}

#[test]
fn lane_key_routes_decodable_events_by_payload_schedule_id() {
    use trogon_decider_runtime::{Event, EventEncode, EventId, EventType, Headers, StreamEvent, StreamPosition};
    use uuid::Uuid;

    let created = v1::ScheduleCreated {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
        schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
        delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
        message: MessageField::some(proto_message(&DomainMessage {
            content: DomainContent::json("{}"),
            headers: DomainHeaders::default(),
        })),
    };
    let event = v1::ScheduleEvent {
        event: Some(created.into()),
    };
    let content = EventEncode::encode(&event).expect("schedule event encodes");
    let r#type = event.event_type().expect("schedule event has a type").to_string();
    let stream_event = StreamEvent {
        stream_id: "wrong-stream".to_string(),
        event: Event {
            id: EventId::new(Uuid::from_u128(1)),
            r#type,
            content,
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
        recorded_at: at_instant(),
    };

    let (key, decoded) = lane_route_from_stream_event(&stream_event);
    assert_eq!(key, ScheduleKey::derive(&ScheduleId::parse("orders/created").unwrap()));
    assert_ne!(key, ScheduleKey::for_stream(&StreamRoutingId::from("wrong-stream")));
    // The decoded change is threaded so the worker does not decode again.
    let DecodedScheduleEvent::Change(change) = decoded else {
        panic!("expected the decoded schedule change to be threaded");
    };
    assert_eq!(change.schedule_id().as_str(), "orders/created");
}

#[test]
fn lane_key_falls_back_to_stream_id_for_foreign_events() {
    use trogon_decider_runtime::{Event, EventId, Headers, StreamEvent, StreamPosition};
    use uuid::Uuid;

    let stream_event = StreamEvent {
        stream_id: "orders/created".to_string(),
        event: Event {
            id: EventId::new(Uuid::from_u128(2)),
            r#type: "foreign.event.v1".to_string(),
            content: b"{}".to_vec(),
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
        recorded_at: at_instant(),
    };

    let (key, decoded) = lane_route_from_stream_event(&stream_event);
    assert_eq!(
        key,
        ScheduleKey::for_stream(&StreamRoutingId::from(stream_event.stream_id.as_str()))
    );
    assert!(matches!(decoded, DecodedScheduleEvent::Foreign));
}
