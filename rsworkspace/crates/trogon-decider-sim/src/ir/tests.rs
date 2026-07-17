use trogon_decider_wit::host;

use super::*;

#[test]
fn wire_envelope_round_trips_through_host_any_envelope() {
    let wire = WireEnvelope::new("trogonai.scheduler.schedules.v1.ScheduleCreated", vec![1, 2, 3]);
    let any = host::AnyEnvelope::from(&wire);
    assert_eq!(any.type_, wire.type_url);
    assert_eq!(any.payload, wire.payload);
    assert_eq!(WireEnvelope::from(any), wire);
}

#[test]
fn wire_envelope_round_trips_through_host_command_envelope() {
    let wire = WireEnvelope::new(
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule",
        vec![4, 5, 6],
    );
    let command = host::CommandEnvelope::from(&wire);
    assert_eq!(command.type_, wire.type_url);
    assert_eq!(command.payload, wire.payload);
    assert_eq!(WireEnvelope::from(command), wire);
}

#[test]
fn new_scenario_has_no_steps_or_stream_id() {
    let scenario = ScenarioIr::new("empty scenario");
    assert_eq!(scenario.name, "empty scenario");
    assert!(scenario.stream_id.is_none());
    assert!(scenario.given.is_empty());
    assert!(scenario.steps.is_empty());
    assert!(scenario.first_command().is_none());
}

#[test]
fn first_command_returns_the_first_steps_when() {
    let mut scenario = ScenarioIr::new("scenario with steps");
    let first = WireEnvelope::new("a", Vec::new());
    let second = WireEnvelope::new("b", Vec::new());
    scenario.steps.push(ScenarioStep {
        when: first.clone(),
        expect: ExpectedOutcome::Accepted,
    });
    scenario.steps.push(ScenarioStep {
        when: second,
        expect: ExpectedOutcome::Rejected,
    });

    assert_eq!(scenario.first_command(), Some(&first));
}

#[test]
fn to_sim_scenario_builds_without_running() {
    let mut scenario = ScenarioIr::new("buildable scenario");
    scenario.steps.push(ScenarioStep {
        when: WireEnvelope::new("a", Vec::new()),
        expect: ExpectedOutcome::Events(vec![WireEnvelope::new("b", Vec::new())]),
    });

    let _sim = scenario.to_sim_scenario();
}
