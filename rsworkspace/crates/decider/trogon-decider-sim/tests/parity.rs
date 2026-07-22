//! Parity between the scheduler schedules native decider and its compiled wasm component.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;

use trogon_decider::Decider;
use trogon_decider_sim::{
    NativeDecideError, NativeDeciderBundle, NativeDomainError, ParityError, ScenarioIr, SimFixture, SimHost,
    WireEnvelope, assert_parity, decode_native_command, native_decide, native_evolve_one,
};
use trogon_decider_test::Suite;
use trogon_scheduler_domain::{CreateSchedule, PauseSchedule, RemoveSchedule, ResumeSchedule};
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, REMOVE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL, state_v1, v1,
};

fn invalid_command(other: &str) -> NativeDomainError {
    NativeDomainError {
        code: "invalid-command".to_string(),
        message: format!("unknown command type '{other}'"),
        details: Vec::new(),
    }
}

/// Mirrors `wasm-components/trogon-schedules-decider`'s `export_decider!` bundle: the same four
/// commands, dispatched by type URL, with `CreateSchedule` as the canonical command whose shared
/// `State`/`Event`/`EvolveError` folds every event regardless of which command produced it.
struct SchedulesBundle;

impl NativeDeciderBundle for SchedulesBundle {
    type State = state_v1::State;

    fn initial_state() -> Self::State {
        trogon_scheduler_domain::initial_state()
    }

    fn stream_id(command: &WireEnvelope) -> Result<String, NativeDomainError> {
        match command.type_url.as_str() {
            CREATE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::CreateSchedule, CreateSchedule>(CREATE_SCHEDULE_TYPE_URL, command)?;
                Ok(command.stream_id().to_string())
            }
            PAUSE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::PauseSchedule, PauseSchedule>(PAUSE_SCHEDULE_TYPE_URL, command)?;
                Ok(command.stream_id().to_string())
            }
            REMOVE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::RemoveSchedule, RemoveSchedule>(REMOVE_SCHEDULE_TYPE_URL, command)?;
                Ok(command.stream_id().to_string())
            }
            RESUME_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::ResumeSchedule, ResumeSchedule>(RESUME_SCHEDULE_TYPE_URL, command)?;
                Ok(command.stream_id().to_string())
            }
            other => Err(invalid_command(other)),
        }
    }

    fn evolve(state: Self::State, event: &WireEnvelope) -> Result<Self::State, NativeDomainError> {
        native_evolve_one::<CreateSchedule>(state, event)
    }

    fn decide(command: &WireEnvelope, state: &Self::State) -> Result<Vec<WireEnvelope>, NativeDecideError> {
        match command.type_url.as_str() {
            CREATE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::CreateSchedule, CreateSchedule>(CREATE_SCHEDULE_TYPE_URL, command)
                        .map_err(NativeDecideError::Faulted)?;
                native_decide(&command, state)
            }
            PAUSE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::PauseSchedule, PauseSchedule>(PAUSE_SCHEDULE_TYPE_URL, command)
                        .map_err(NativeDecideError::Faulted)?;
                native_decide(&command, state)
            }
            REMOVE_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::RemoveSchedule, RemoveSchedule>(REMOVE_SCHEDULE_TYPE_URL, command)
                        .map_err(NativeDecideError::Faulted)?;
                native_decide(&command, state)
            }
            RESUME_SCHEDULE_TYPE_URL => {
                let command =
                    decode_native_command::<v1::ResumeSchedule, ResumeSchedule>(RESUME_SCHEDULE_TYPE_URL, command)
                        .map_err(NativeDecideError::Faulted)?;
                native_decide(&command, state)
            }
            other => Err(NativeDecideError::Faulted(invalid_command(other))),
        }
    }
}

fn schedules_suite() -> Suite {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../cli/trogon-decider-test/schedules.yaml"
    );
    let yaml = fs::read_to_string(path).expect("read schedules.yaml");
    Suite::from_yaml(&yaml).expect("parse schedules.yaml")
}

/// Every scenario in the CLI's `schedules.yaml`, converted through the same
/// `Suite`/`Scenario::to_ir` code path the `decider-test` binary uses, run through both the
/// native scheduler schedules decider and its compiled wasm component. A codec bug, a codegen
/// bug, or a WIT regression that only breaks one of the two paths fails this test even if each
/// runner separately satisfies the scenario's own declared expectations.
#[test]
fn schedules_yaml_scenarios_have_native_wasm_parity() {
    let suite = schedules_suite();
    let scenarios = suite.to_ir().expect("schedules.yaml converts to ir");
    assert!(!scenarios.is_empty(), "schedules.yaml declares no scenarios");

    let names: BTreeSet<&str> = scenarios.iter().map(|scenario| scenario.name.as_str()).collect();
    assert!(
        names.contains("create, pause, then resume a schedule across one session"),
        "expected the multi-step create/pause/resume scenario to still be declared"
    );
    assert!(
        names.iter().any(|name| name.starts_with("reject ")),
        "expected at least one rejection scenario to still be declared"
    );

    let wasm = SimFixture::schedules().bytes().to_vec();
    let host = SimHost::load(&wasm).expect("load schedules wasm component");

    for scenario in &scenarios {
        let mut instance = host.instantiate(()).expect("instantiate schedules component");
        assert_parity::<SchedulesBundle, _>(scenario, &mut instance)
            .unwrap_or_else(|error| panic!("scenario '{}' diverged: {error}", scenario.name));
    }
}

fn create_backup_scenario() -> ScenarioIr {
    let suite = schedules_suite();
    suite
        .to_ir()
        .expect("schedules.yaml converts to ir")
        .into_iter()
        .find(|scenario| scenario.name == "create schedule from initial state")
        .expect("schedules.yaml declares the initial-state create scenario")
}

/// `assert_parity` resolves the scenario's first command's stream id through both runners and,
/// when the scenario declares one, checks the resolved id matches it too.
#[test]
fn assert_parity_passes_when_declared_stream_id_matches_both_runners() {
    let mut scenario = create_backup_scenario();
    scenario.stream_id = Some("backup".to_string());

    let wasm = SimFixture::schedules().bytes().to_vec();
    let host = SimHost::load(&wasm).expect("load schedules wasm component");
    let mut instance = host.instantiate(()).expect("instantiate schedules component");

    assert_parity::<SchedulesBundle, _>(&scenario, &mut instance)
        .expect("declared stream id must match both runners' resolved id");
}

#[test]
fn assert_parity_reports_a_declared_stream_id_mismatch() {
    let mut scenario = create_backup_scenario();
    scenario.stream_id = Some("not-backup".to_string());

    let wasm = SimFixture::schedules().bytes().to_vec();
    let host = SimHost::load(&wasm).expect("load schedules wasm component");
    let mut instance = host.instantiate(()).expect("instantiate schedules component");

    let error = assert_parity::<SchedulesBundle, _>(&scenario, &mut instance).unwrap_err();
    assert!(
        matches!(&error, ParityError::StreamIdDeclaredMismatch { .. }),
        "{error}"
    );
}
