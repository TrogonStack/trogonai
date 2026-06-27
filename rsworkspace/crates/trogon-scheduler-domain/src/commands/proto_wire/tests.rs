use buffa::MessageField;
use trogonai_proto::scheduler::schedules::v1;

use super::*;

fn minimal_create_proto(id: &str) -> v1::CreateScheduleCommand {
    v1::CreateScheduleCommand {
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
        PauseSchedule::try_from(v1::PauseScheduleCommand {
            schedule_id: String::new(),
        })
        .is_err()
    );
}
