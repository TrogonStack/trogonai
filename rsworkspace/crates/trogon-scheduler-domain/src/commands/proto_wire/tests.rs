use buffa::MessageField;
use trogonai_proto::scheduler::schedules::v1;

use super::*;

fn minimal_create_proto(id: &str) -> v1::CreateSchedule {
    v1::CreateSchedule {
        schedule_id: id.to_string(),
        status: MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Scheduled {}.into()),
        }),
        schedule: MessageField::some(v1::Schedule {
            kind: Some(
                v1::schedule::Every {
                    every: MessageField::some(buffa_types::google::protobuf::Duration {
                        seconds: 30,
                        nanos: 0,
                        ..buffa_types::google::protobuf::Duration::default()
                    }),
                }
                .into(),
            ),
        }),
        delivery: MessageField::some(v1::Delivery {
            kind: Some(
                v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::none(),
                    source: MessageField::none(),
                }
                .into(),
            ),
        }),
        message: MessageField::some(v1::Message {
            content: MessageField::some(content_v1alpha1::Content {
                content_type: "application/json".to_string(),
                data: br#"{"kind":"heartbeat"}"#.to_vec(),
            }),
            headers: Vec::new(),
        }),
    }
}

#[test]
fn create_schedule_command_round_trips_from_proto() {
    let command = CreateSchedule::try_from(minimal_create_proto("backup")).unwrap();
    assert_eq!(command.id.as_str(), "backup");
    assert_eq!(command.status, ScheduleEventStatus::Scheduled);
}

#[test]
fn pause_command_requires_schedule_id() {
    assert!(
        PauseSchedule::try_from(v1::PauseSchedule {
            schedule_id: String::new(),
        })
        .is_err()
    );
}

fn timestamp(seconds: i64) -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp {
        seconds,
        nanos: 0,
        ..buffa_types::google::protobuf::Timestamp::default()
    }
}

fn timezone(id: &str) -> trogonai_proto::google::r#type::TimeZone {
    trogonai_proto::google::r#type::TimeZone {
        id: id.to_string(),
        ..trogonai_proto::google::r#type::TimeZone::default()
    }
}

fn create_proto_with_schedule(kind: v1::schedule::Kind) -> v1::CreateSchedule {
    let mut proto = minimal_create_proto("backup");
    proto.schedule = MessageField::some(v1::Schedule { kind: Some(kind) });
    proto
}

#[test]
fn create_with_at_schedule_round_trips() {
    let command = CreateSchedule::try_from(create_proto_with_schedule(
        v1::schedule::At {
            at: MessageField::some(timestamp(1_700_000_000)),
        }
        .into(),
    ))
    .unwrap();
    assert!(matches!(command.schedule, Schedule::At { .. }));
}

#[test]
fn create_with_cron_schedule_round_trips() {
    let command = CreateSchedule::try_from(create_proto_with_schedule(
        v1::schedule::Cron {
            expr: "0 0 * * * *".to_string(),
            timezone: MessageField::some(timezone("UTC")),
        }
        .into(),
    ))
    .unwrap();
    assert!(matches!(command.schedule, Schedule::Cron { .. }));
}

#[test]
fn create_with_rrule_schedule_round_trips() {
    let command = CreateSchedule::try_from(create_proto_with_schedule(
        v1::schedule::RRule {
            dtstart: MessageField::some(timestamp(1_700_000_000)),
            rrule: "FREQ=DAILY;COUNT=2".to_string(),
            timezone: MessageField::some(timezone("UTC")),
            rdate: vec![timestamp(1_700_100_000)],
            exdate: vec![timestamp(1_700_200_000)],
        }
        .into(),
    ))
    .unwrap();
    assert!(matches!(command.schedule, Schedule::RRule { .. }));
}

#[test]
fn create_with_paused_status_round_trips() {
    let mut proto = minimal_create_proto("backup");
    proto.status = MessageField::some(v1::ScheduleStatus {
        kind: Some(v1::schedule_status::Paused {}.into()),
    });
    let command = CreateSchedule::try_from(proto).unwrap();
    assert_eq!(command.status, ScheduleEventStatus::Paused);
}

#[test]
fn create_with_delivery_ttl_and_source_round_trips() {
    let mut proto = minimal_create_proto("backup");
    proto.delivery = MessageField::some(v1::Delivery {
        kind: Some(
            v1::delivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl: MessageField::some(buffa_types::google::protobuf::Duration {
                    seconds: 60,
                    nanos: 0,
                    ..buffa_types::google::protobuf::Duration::default()
                }),
                source: MessageField::some(v1::delivery::nats_message::Source {
                    kind: Some(
                        v1::delivery::nats_message::LatestFromSubject {
                            subject: "agent.events".to_string(),
                        }
                        .into(),
                    ),
                }),
            }
            .into(),
        ),
    });
    let command = CreateSchedule::try_from(proto).unwrap();
    assert!(matches!(command.delivery, Delivery::NatsEvent { .. }));
}

#[test]
fn create_with_non_json_content_and_headers_round_trips() {
    let mut proto = minimal_create_proto("backup");
    proto.message = MessageField::some(v1::Message {
        content: MessageField::some(content_v1alpha1::Content {
            content_type: "text/plain".to_string(),
            data: b"hello".to_vec(),
        }),
        headers: vec![v1::Header {
            name: "x-trace".to_string(),
            value: "abc".to_string(),
        }],
    });
    assert!(CreateSchedule::try_from(proto).is_ok());
}

#[test]
fn create_rejects_empty_schedule_id() {
    let mut proto = minimal_create_proto("backup");
    proto.schedule_id = String::new();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("schedule_id"))
    ));
}

#[test]
fn create_rejects_missing_status() {
    let mut proto = minimal_create_proto("backup");
    proto.status = MessageField::none();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("status"))
    ));
}

#[test]
fn create_rejects_status_without_kind() {
    let mut proto = minimal_create_proto("backup");
    proto.status = MessageField::some(v1::ScheduleStatus { kind: None });
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::InvalidStatus)
    ));
}

#[test]
fn create_rejects_missing_schedule() {
    let mut proto = minimal_create_proto("backup");
    proto.schedule = MessageField::none();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("schedule"))
    ));
}

#[test]
fn create_rejects_schedule_without_kind() {
    let proto = create_proto_with_schedule_none();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("schedule.kind"))
    ));
}

fn create_proto_with_schedule_none() -> v1::CreateSchedule {
    let mut proto = minimal_create_proto("backup");
    proto.schedule = MessageField::some(v1::Schedule { kind: None });
    proto
}

#[test]
fn create_rejects_missing_delivery() {
    let mut proto = minimal_create_proto("backup");
    proto.delivery = MessageField::none();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("delivery"))
    ));
}

#[test]
fn create_rejects_delivery_without_nats_message() {
    let mut proto = minimal_create_proto("backup");
    proto.delivery = MessageField::some(v1::Delivery { kind: None });
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("delivery.nats_message"))
    ));
}

#[test]
fn create_rejects_source_without_kind() {
    let mut proto = minimal_create_proto("backup");
    proto.delivery = MessageField::some(v1::Delivery {
        kind: Some(
            v1::delivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl: MessageField::none(),
                source: MessageField::some(v1::delivery::nats_message::Source { kind: None }),
            }
            .into(),
        ),
    });
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("delivery.source.kind"))
    ));
}

#[test]
fn create_rejects_missing_message() {
    let mut proto = minimal_create_proto("backup");
    proto.message = MessageField::none();
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("message"))
    ));
}

#[test]
fn create_rejects_missing_message_content() {
    let mut proto = minimal_create_proto("backup");
    proto.message = MessageField::some(v1::Message {
        content: MessageField::none(),
        headers: Vec::new(),
    });
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("message.content"))
    ));
}

#[test]
fn create_rejects_missing_at_timestamp() {
    let proto = create_proto_with_schedule(
        v1::schedule::At {
            at: MessageField::none(),
        }
        .into(),
    );
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("schedule.at"))
    ));
}

#[test]
fn create_rejects_missing_every_duration() {
    let proto = create_proto_with_schedule(
        v1::schedule::Every {
            every: MessageField::none(),
        }
        .into(),
    );
    assert!(matches!(
        CreateSchedule::try_from(proto),
        Err(CommandWireError::MissingField("schedule.every"))
    ));
}

#[test]
fn pause_command_round_trips() {
    let command = PauseSchedule::try_from(v1::PauseSchedule {
        schedule_id: "backup".to_string(),
    })
    .unwrap();
    assert_eq!(command.id.as_str(), "backup");
}

#[test]
fn remove_command_round_trips() {
    assert!(
        RemoveSchedule::try_from(v1::RemoveSchedule {
            schedule_id: "backup".to_string(),
        })
        .is_ok()
    );
}

#[test]
fn remove_command_requires_schedule_id() {
    assert!(
        RemoveSchedule::try_from(v1::RemoveSchedule {
            schedule_id: String::new(),
        })
        .is_err()
    );
}

#[test]
fn resume_command_round_trips() {
    assert!(
        ResumeSchedule::try_from(v1::ResumeSchedule {
            schedule_id: "backup".to_string(),
        })
        .is_ok()
    );
}

#[test]
fn resume_command_requires_schedule_id() {
    assert!(
        ResumeSchedule::try_from(v1::ResumeSchedule {
            schedule_id: String::new(),
        })
        .is_err()
    );
}
