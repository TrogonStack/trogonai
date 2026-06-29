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

fn pause_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: PAUSE_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::PauseSchedule {
            schedule_id: id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn unknown_command() -> CommandEnvelope {
    CommandEnvelope {
        type_: "type.googleapis.com/trogonai.scheduler.schedules.v1.DoesNotExist".to_string(),
        payload: Vec::new(),
    }
}

#[test]
fn schedules_fixture_has_no_external_imports() {
    assert_zero_imports(&schedules_wasm()).expect("schedules decider must have zero world imports");
}

#[test]
fn descriptor_lists_create_command() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let descriptor = instance.descriptor().unwrap();
    assert_eq!(descriptor.name, "scheduler.schedules");
    assert!(
        descriptor
            .commands
            .iter()
            .any(|spec| spec.command_type == CREATE_SCHEDULE_TYPE_URL)
    );
}

#[test]
fn stream_id_returns_schedule_id() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let stream = instance.stream_id(&create_command("backup")).unwrap().unwrap();
    assert_eq!(stream, "backup");
}

#[test]
fn then_accepted_passes_for_create() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(create_command("backup"))
        .then_accepted()
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_accepted_reports_rejection_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(pause_command("missing"))
        .then_accepted()
        .run(&mut instance)
        .unwrap_err();
    assert!(
        error.to_string().contains("expected acceptance, got rejection"),
        "{error}"
    );
}

#[test]
fn then_accepted_reports_fault_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_accepted()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("expected acceptance, got fault"), "{error}");
}

#[test]
fn then_error_matches_fault_code() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(unknown_command())
        .then_error("invalid-command")
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_error_matches_rejection() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(pause_command("missing"))
        .then_error("rejected")
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_error_reports_rejection_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(pause_command("missing"))
        .then_error("some-other-code")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("got rejection: rejected"), "{error}");
}

#[test]
fn then_error_reports_fault_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_error("some-other-code")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("got fault: invalid-command"), "{error}");
}

#[test]
fn then_error_reports_unexpected_events() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .then_error("rejected")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("got 1 event(s)"), "{error}");
}

#[test]
fn then_rejected_passes_for_missing_pause() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(pause_command("missing"))
        .then_rejected()
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_rejected_reports_event_count() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    assert!(
        error.to_string().contains("expected rejection, got 1 event(s)"),
        "{error}"
    );
}

#[test]
fn then_rejected_reports_fault_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("expected rejection, got fault"), "{error}");
}

#[test]
fn then_events_reports_count_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .then_events([])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("expected 0 event(s), got 1"), "{error}");
}

#[test]
fn then_events_reports_payload_mismatch() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .then_events([schedule_created_event("other")])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("event 0 mismatch"), "{error}");
}

#[test]
fn then_events_reports_fault() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_events([schedule_created_event("backup")])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("faulted: invalid-command"), "{error}");
}

#[test]
fn run_requires_a_command() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new().then_rejected().run(&mut instance).unwrap_err();
    assert!(error.to_string().contains("scenario missing .when(...)"), "{error}");
}

#[test]
fn run_requires_a_then_expectation() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .run(&mut instance)
        .unwrap_err();
    assert!(
        error.to_string().contains("scenario missing .then_events(...)"),
        "{error}"
    );
}

#[test]
fn then_events_reports_rejection() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(pause_command("missing"))
        .then_events([schedule_created_event("backup")])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.to_string().contains("rejected:"), "{error}");
}

#[test]
fn default_scenario_matches_new() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::default()
        .when(create_command("backup"))
        .then_accepted()
        .run(&mut instance)
        .unwrap();
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
