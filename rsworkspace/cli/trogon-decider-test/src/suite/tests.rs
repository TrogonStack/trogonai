use super::*;

fn error_expectation(yaml: &str) -> String {
    match serde_yaml::from_str::<Then>(yaml) {
        Ok(Then::Error { error }) => error.expected().unwrap_or_default(),
        _ => String::new(),
    }
}

#[test]
fn then_error_accepts_structured_code() {
    assert_eq!(error_expectation("error:\n  code: already-exists\n"), "already-exists");
}

#[test]
fn then_error_accepts_plain_string() {
    assert_eq!(error_expectation("error: already-exists\n"), "already-exists");
}

fn scenario(when: Option<serde_json::Value>, then: Option<Then>, steps: Option<Vec<Step>>) -> Scenario {
    Scenario {
        name: "scenario under test".to_string(),
        given: Vec::new(),
        when,
        then,
        steps,
    }
}

#[test]
fn steps_normalizes_legacy_when_then_shape() {
    let scenario = scenario(
        Some(serde_json::json!({"@type": "a"})),
        Some(Then::Rejected { rejected: true }),
        None,
    );
    assert_eq!(scenario.steps().expect("valid scenario").len(), 1);
}

#[test]
fn steps_returns_ordered_step_list() {
    let scenario = scenario(
        None,
        None,
        Some(vec![
            Step {
                when: serde_json::json!({"@type": "a"}),
                then: Then::Rejected { rejected: true },
            },
            Step {
                when: serde_json::json!({"@type": "b"}),
                then: Then::Rejected { rejected: false },
            },
        ]),
    );
    assert_eq!(scenario.steps().expect("valid scenario").len(), 2);
}

#[test]
fn steps_rejects_empty_steps_list() {
    let scenario = scenario(None, None, Some(Vec::new()));
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("empty steps list"), "unexpected error: {error}");
}

#[test]
fn steps_rejects_mixing_steps_and_when_then() {
    let scenario = scenario(
        Some(serde_json::json!({"@type": "a"})),
        Some(Then::Rejected { rejected: true }),
        Some(vec![Step {
            when: serde_json::json!({"@type": "a"}),
            then: Then::Rejected { rejected: true },
        }]),
    );
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("exactly one of"), "unexpected error: {error}");
}

#[test]
fn steps_rejects_missing_when_and_then() {
    let scenario = scenario(None, None, None);
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("exactly one of"), "unexpected error: {error}");
}

#[test]
fn to_ir_converts_given_steps_and_events() {
    let scenario = Scenario {
        name: "create schedule".to_string(),
        given: Vec::new(),
        when: None,
        then: None,
        steps: Some(vec![Step {
            when: serde_json::json!({
                "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule",
                "schedule_id": "backup",
                "status": { "scheduled": {} },
                "schedule": { "every": { "every": "30s" } },
                "delivery": { "nats_message": { "subject": "agent.run" } },
                "message": { "content": { "content_type": "application/json", "data": "e30=" } },
            }),
            then: Then::Events {
                events: vec![serde_json::json!({
                    "@type": "trogonai.scheduler.schedules.v1.ScheduleCreated",
                    "schedule_id": "backup",
                    "status": { "scheduled": {} },
                    "schedule": { "every": { "every": "30s" } },
                    "delivery": { "nats_message": { "subject": "agent.run" } },
                    "message": { "content": { "content_type": "application/json", "data": "e30=" } },
                })],
            },
        }]),
    };

    let ir = scenario.to_ir().expect("scenario converts to ir");
    assert_eq!(ir.name, "create schedule");
    assert!(ir.given.is_empty());
    assert_eq!(ir.steps.len(), 1);
    assert_eq!(
        ir.steps[0].when.type_url,
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule"
    );
    assert!(matches!(&ir.steps[0].expect, ExpectedOutcome::Events(events) if events.len() == 1));
}

#[test]
fn to_ir_maps_rejected_and_accepted() {
    let rejected = scenario(
        Some(serde_json::json!({
            "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.PauseSchedule",
            "schedule_id": "backup",
        })),
        Some(Then::Rejected { rejected: true }),
        None,
    );
    let ir = rejected.to_ir().expect("scenario converts to ir");
    assert!(matches!(ir.steps[0].expect, ExpectedOutcome::Rejected));

    let accepted = scenario(
        Some(serde_json::json!({
            "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.PauseSchedule",
            "schedule_id": "backup",
        })),
        Some(Then::Rejected { rejected: false }),
        None,
    );
    let ir = accepted.to_ir().expect("scenario converts to ir");
    assert!(matches!(ir.steps[0].expect, ExpectedOutcome::Accepted));
}
