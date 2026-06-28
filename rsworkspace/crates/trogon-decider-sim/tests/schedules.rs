//! Integration tests against the scheduler schedules WASM bundle.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use buffa::Message as _;
use buffa::MessageField;
use buffa::MessageName as _;
use trogon_decider_sim::{SimFixture, SimHost, SimScenario, assert_zero_imports};
use trogon_decider_wit::host::{self, CommandEnvelope};
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, v1};

fn schedules_wasm() -> Vec<u8> {
    SimFixture::schedules().bytes().to_vec()
}

fn create_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: CREATE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::CreateSchedule {
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
        .encode_to_vec(),
    }
}

fn schedule_created_event(id: &str) -> host::AnyEnvelope {
    let _ = create_command(id);
    host::AnyEnvelope {
        type_: v1::ScheduleCreated::FULL_NAME.to_string(),
        payload: v1::ScheduleCreated {
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
        .encode_to_vec(),
    }
}

#[test]
fn schedules_fixture_has_no_external_imports() {
    assert_zero_imports(&schedules_wasm()).expect("schedules decider must have zero world imports");
}

#[test]
fn create_schedule_from_initial_state() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(create_command("backup"))
        .then_events([schedule_created_event("backup")])
        .run(&mut instance)
        .unwrap();
}

#[test]
fn pause_existing_schedule() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .given([schedule_created_event("backup")])
        .when(CommandEnvelope {
            type_: PAUSE_SCHEDULE_TYPE_URL.to_string(),
            payload: v1::PauseSchedule {
                schedule_id: "backup".to_string(),
            }
            .encode_to_vec(),
        })
        .then_events([host::AnyEnvelope {
            type_: v1::SchedulePaused::FULL_NAME.to_string(),
            payload: v1::SchedulePaused {
                schedule_id: "backup".to_string(),
            }
            .encode_to_vec(),
        }])
        .run(&mut instance)
        .unwrap();
}
