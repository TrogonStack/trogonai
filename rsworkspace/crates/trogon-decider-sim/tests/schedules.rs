//! Integration tests against the scheduler schedules WASM bundle.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use buffa::Message as _;
use buffa::MessageField;
use buffa::MessageName as _;
use trogon_decider_sim::{
    ExpectedOutcome, ScenarioError, ScenarioIr, ScenarioStep, SimError, SimFixture, SimHost, SimScenario,
    StreamIdOutcome, WasmEngineConfig, WireEnvelope, assert_zero_imports,
};
use trogon_decider_wit::host::{self, CommandEnvelope};
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL, v1,
};

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

fn resume_command(id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: RESUME_SCHEDULE_TYPE_URL.to_string(),
        payload: v1::ResumeSchedule {
            schedule_id: id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn schedule_paused_event(id: &str) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: v1::SchedulePaused::FULL_NAME.to_string(),
        payload: v1::SchedulePaused {
            schedule_id: id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn schedule_resumed_event(id: &str) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: v1::ScheduleResumed::FULL_NAME.to_string(),
        payload: v1::ScheduleResumed {
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
fn run_wasm_resolves_the_first_steps_stream_id() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let mut scenario = ScenarioIr::new("resolve stream id");
    scenario.steps.push(ScenarioStep {
        when: WireEnvelope::from(create_command("backup")),
        expect: ExpectedOutcome::Accepted,
    });

    let run = scenario.run_wasm(&mut instance).unwrap();
    assert_eq!(run.stream_id, Some(StreamIdOutcome::Resolved("backup".to_string())));
}

#[test]
fn run_wasm_reports_no_stream_id_for_an_empty_scenario() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let scenario = ScenarioIr::new("no steps");
    let run = scenario.run_wasm(&mut instance).unwrap();
    assert!(run.stream_id.is_none());
}

#[test]
fn sim_host_applies_the_production_engine_config_by_default() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    assert_eq!(host.config(), WasmEngineConfig::default());
}

#[test]
fn a_runaway_decider_traps_on_fuel_exhaustion_like_production_would() {
    let config = WasmEngineConfig::default().with_fuel_per_call(1);
    let host = SimHost::load_with_config(&schedules_wasm(), config).unwrap();

    // Every guest call is armed with this same starved fuel budget (mirroring
    // production's `arm_guest_call`), so either instantiation or the first
    // guest export call must trap with `OutOfFuel`, not run unbounded.
    let trap = match host.instantiate(()) {
        Err(SimError::Instantiate { source }) => source,
        Err(other) => panic!("expected SimError::Instantiate on fuel exhaustion, got {other}"),
        Ok(mut instance) => match instance.descriptor() {
            Err(source) => source,
            Ok(descriptor) => panic!("expected a fuel-exhaustion trap, but the guest call succeeded: {descriptor:?}"),
        },
    };
    assert!(
        matches!(trap.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::OutOfFuel)),
        "expected an OutOfFuel trap, got {trap}"
    );
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
        matches!(&error, ScenarioError::AcceptanceGotRejection { .. }),
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
    assert!(matches!(&error, ScenarioError::AcceptanceGotFault { .. }), "{error}");
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
    assert!(
        matches!(&error, ScenarioError::ErrorGotRejection { code, .. } if code.as_str() == "rejected"),
        "{error}"
    );
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
    assert!(
        matches!(&error, ScenarioError::ErrorGotFault { code, .. } if code.as_str() == "invalid-command"),
        "{error}"
    );
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
    assert!(
        matches!(&error, ScenarioError::ErrorGotEvents { count: 1, .. }),
        "{error}"
    );
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
        matches!(&error, ScenarioError::RejectionGotEvents { count: 1 }),
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
    assert!(matches!(&error, ScenarioError::RejectionGotFault { .. }), "{error}");
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
    assert!(
        matches!(&error, ScenarioError::EventCountMismatch { expected: 0, actual: 1 }),
        "{error}"
    );
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
    assert!(
        matches!(&error, ScenarioError::EventMismatch { index: 0, .. }),
        "{error}"
    );
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
    assert!(
        matches!(&error, ScenarioError::EventsGotFault { code, .. } if code.as_str() == "invalid-command"),
        "{error}"
    );
}

#[test]
fn run_requires_a_command() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new().then_rejected().run(&mut instance).unwrap_err();
    assert!(matches!(&error, ScenarioError::MissingWhen), "{error}");
}

#[test]
fn run_requires_a_then_expectation() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .run(&mut instance)
        .unwrap_err();
    assert!(matches!(&error, ScenarioError::MissingExpectation), "{error}");
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
    assert!(matches!(&error, ScenarioError::EventsGotRejection { .. }), "{error}");
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

#[test]
fn evolve_skips_events_outside_this_deciders_set() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    // An envelope outside the schedules event set must be skipped without affecting state
    // (matching the native runtime replay), so a fresh create still succeeds afterward.
    let foreign = host::AnyEnvelope {
        type_: "trogonai.scheduler.schedules.v1.NotAnEvent".to_string(),
        payload: vec![1, 2, 3],
    };
    SimScenario::new()
        .given([foreign])
        .when(create_command("backup"))
        .then_events([schedule_created_event("backup")])
        .run(&mut instance)
        .unwrap();
}

#[test]
fn multi_step_scenario_feeds_events_forward_within_one_session() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    // Each step's `then_events` output is folded into the same open session
    // before the next step's command is decided, so resume only succeeds
    // because it observes the pause this scenario decided one step earlier.
    SimScenario::new()
        .when(create_command("backup"))
        .then_events([schedule_created_event("backup")])
        .when(pause_command("backup"))
        .then_events([schedule_paused_event("backup")])
        .when(resume_command("backup"))
        .then_events([schedule_resumed_event("backup")])
        .run(&mut instance)
        .unwrap();
}

#[test]
fn multi_step_scenario_reports_which_step_failed() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(create_command("backup"))
        .then_events([schedule_created_event("backup")])
        .when(pause_command("backup"))
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    match &error {
        ScenarioError::Step { index, source } => {
            assert_eq!(*index, 1);
            assert!(matches!(
                source.as_ref(),
                ScenarioError::RejectionGotEvents { count: 1 }
            ));
        }
        other => panic!("expected ScenarioError::Step, got {other}"),
    }
}

#[test]
fn snapshot_round_trips_into_a_restored_session() {
    let host = SimHost::load(&schedules_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    // Fold a creation into one session, then capture its snapshot.
    let snapshot = {
        let mut session = instance.open_session(None).unwrap();
        session.evolve(&[schedule_created_event("backup")]).unwrap().unwrap();
        session.snapshot().unwrap()
    };
    assert!(snapshot.is_some(), "guest should produce a snapshot frame");

    // A fresh session restored from that snapshot must already see the schedule as present,
    // so re-creating it is rejected without replaying any events.
    let mut restored = instance.open_session(snapshot.as_deref()).unwrap();
    let outcome = restored.decide(&create_command("backup")).unwrap();
    assert!(matches!(outcome, Err(host::DecideError::Rejected(_))), "{outcome:?}");
}
