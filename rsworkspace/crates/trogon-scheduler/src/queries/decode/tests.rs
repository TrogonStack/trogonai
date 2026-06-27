use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};

use super::*;

fn ts(seconds: i64) -> Timestamp {
    Timestamp {
        seconds,
        ..Default::default()
    }
}

fn view() -> projections_v1::ScheduleProjection {
    projections_v1::ScheduleProjection {
        schedule_id: "orders/created".to_string(),
        status: MessageField::some(projections_v1::ScheduleStatus {
            kind: Some(projections_v1::schedule_status::Paused {}.into()),
        }),
        completed: Some(true),
        next_occurrence_at: MessageField::some(ts(1_700_000_100)),
        last_occurrence_at: MessageField::some(ts(1_700_000_000)),
        schedule: MessageField::some(projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: 1,
                        nanos: 500_000_000,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }),
        delivery: MessageField::some(projections_v1::Delivery {
            kind: Some(
                projections_v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::some(Duration {
                        seconds: 30,
                        ..Default::default()
                    }),
                    source: MessageField::none(),
                }
                .into(),
            ),
        }),
        message: MessageField::some(projections_v1::Message {
            content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "text/plain".to_string(),
                data: b"hello".to_vec(),
            }),
            headers: Vec::new(),
        }),
    }
}

fn timezone(id: &str) -> trogonai_proto::google::r#type::TimeZone {
    trogonai_proto::google::r#type::TimeZone {
        id: id.to_string(),
        ..Default::default()
    }
}

fn with_schedule(kind: projections_v1::__buffa::oneof::schedule::Kind) -> projections_v1::ScheduleProjection {
    let mut view = view();
    view.schedule = MessageField::some(projections_v1::Schedule { kind: Some(kind) });
    view
}

#[test]
fn decodes_every_field_of_the_view() {
    let schedule = schedule_from_view(&view()).unwrap();

    assert_eq!(schedule.id, "orders/created");
    assert_eq!(schedule.status, ScheduleEventStatus::Paused);
    assert!(schedule.completed);
    assert!(schedule.next_occurrence_at.is_some());
    assert!(schedule.last_occurrence_at.is_some());
    assert_eq!(
        schedule.schedule,
        ScheduleEventSchedule::Every {
            every: std::time::Duration::from_millis(1500),
        }
    );
    assert_eq!(schedule.message.content.content_type(), "text/plain");
    assert_eq!(schedule.message.content.as_str(), "hello");
}

#[test]
fn decode_schedule_round_trips_encoded_bytes() {
    let bytes = buffa::Message::encode_to_vec(&view());
    let schedule = decode_schedule(&bytes).unwrap();
    assert_eq!(schedule.id, "orders/created");
}

#[test]
fn decode_schedule_rejects_corrupt_bytes() {
    let error = decode_schedule(&[0xff, 0xff, 0xff]).unwrap_err();
    assert!(matches!(error, SchedulerError::Kv { .. }));
}

#[test]
fn decodes_scheduled_status() {
    let mut view = view();
    view.status = MessageField::some(projections_v1::ScheduleStatus {
        kind: Some(projections_v1::schedule_status::Scheduled {}.into()),
    });
    assert_eq!(
        schedule_from_view(&view).unwrap().status,
        ScheduleEventStatus::Scheduled
    );
}

#[test]
fn decodes_at_schedule() {
    let view = with_schedule(
        projections_v1::schedule::At {
            at: MessageField::some(ts(1_700_000_000)),
        }
        .into(),
    );
    assert!(matches!(
        schedule_from_view(&view).unwrap().schedule,
        ScheduleEventSchedule::At { .. }
    ));
}

#[test]
fn decodes_cron_schedule_with_timezone() {
    let view = with_schedule(
        projections_v1::schedule::Cron {
            expr: "0 * * * *".to_string(),
            timezone: MessageField::some(timezone("UTC")),
        }
        .into(),
    );
    match schedule_from_view(&view).unwrap().schedule {
        ScheduleEventSchedule::Cron { expr, timezone } => {
            assert_eq!(expr, "0 * * * *");
            assert_eq!(timezone.as_deref(), Some("UTC"));
        }
        other => panic!("expected cron, got {other:?}"),
    }
}

#[test]
fn decodes_rrule_schedule() {
    let view = with_schedule(
        projections_v1::schedule::RRule {
            dtstart: MessageField::some(ts(1_700_000_000)),
            rrule: "FREQ=DAILY".to_string(),
            timezone: MessageField::some(timezone("UTC")),
            rdate: vec![ts(1_700_000_100)],
            exdate: vec![ts(1_700_000_200)],
        }
        .into(),
    );
    match schedule_from_view(&view).unwrap().schedule {
        ScheduleEventSchedule::RRule {
            rrule, rdate, exdate, ..
        } => {
            assert_eq!(rrule, "FREQ=DAILY");
            assert_eq!(rdate.len(), 1);
            assert_eq!(exdate.len(), 1);
        }
        other => panic!("expected rrule, got {other:?}"),
    }
}

#[test]
fn decodes_delivery_with_sampling_source() {
    let mut view = view();
    view.delivery = MessageField::some(projections_v1::Delivery {
        kind: Some(
            projections_v1::delivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl: MessageField::none(),
                source: MessageField::some(projections_v1::delivery::nats_message::Source {
                    kind: Some(
                        projections_v1::delivery::nats_message::LatestFromSubject {
                            subject: "sensors.temp".to_string(),
                        }
                        .into(),
                    ),
                }),
            }
            .into(),
        ),
    });
    match schedule_from_view(&view).unwrap().delivery {
        ScheduleEventDelivery::NatsMessage { source, .. } => {
            assert!(matches!(
                source,
                Some(ScheduleEventSamplingSource::LatestFromSubject { .. })
            ));
        }
    }
}

#[test]
fn rejects_missing_required_fields() {
    let mut missing_schedule = view();
    missing_schedule.schedule = MessageField::none();
    assert!(schedule_from_view(&missing_schedule).is_err());

    let mut missing_delivery = view();
    missing_delivery.delivery = MessageField::none();
    assert!(schedule_from_view(&missing_delivery).is_err());

    let mut missing_message = view();
    missing_message.message = MessageField::none();
    assert!(schedule_from_view(&missing_message).is_err());
}

#[test]
fn rejects_schedule_and_delivery_without_a_case() {
    let no_schedule_case = with_schedule_none();
    assert!(schedule_from_view(&no_schedule_case).is_err());

    let mut no_delivery_case = view();
    no_delivery_case.delivery = MessageField::some(projections_v1::Delivery { kind: None });
    assert!(schedule_from_view(&no_delivery_case).is_err());
}

fn with_schedule_none() -> projections_v1::ScheduleProjection {
    let mut view = view();
    view.schedule = MessageField::some(projections_v1::Schedule { kind: None });
    view
}

#[test]
fn rejects_non_utf8_message_content() {
    let mut view = view();
    view.message = MessageField::some(projections_v1::Message {
        content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: "application/octet-stream".to_string(),
            data: vec![0xff, 0xfe],
        }),
        headers: Vec::new(),
    });
    assert!(schedule_from_view(&view).is_err());
}

#[test]
fn rejects_sampling_source_without_a_case() {
    let mut view = view();
    view.delivery = MessageField::some(projections_v1::Delivery {
        kind: Some(
            projections_v1::delivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl: MessageField::none(),
                source: MessageField::some(projections_v1::delivery::nats_message::Source { kind: None }),
            }
            .into(),
        ),
    });
    assert!(schedule_from_view(&view).is_err());
}

#[test]
fn message_without_content_defaults_to_empty() {
    let mut view = view();
    view.message = MessageField::some(projections_v1::Message {
        content: MessageField::none(),
        headers: Vec::new(),
    });
    let schedule = schedule_from_view(&view).unwrap();
    assert_eq!(schedule.message.content.as_str(), "");
}
