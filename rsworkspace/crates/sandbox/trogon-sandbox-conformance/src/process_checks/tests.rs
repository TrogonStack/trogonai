use std::error::Error;

use serde_json::json;

use super::*;

fn listing(processes: serde_json::Value) -> Result<ListProcessesResponse, Box<dyn Error>> {
    Ok(serde_json::from_value(
        json!({ "outcome": "processes", "processes": processes }),
    )?)
}

fn process(name: &str, id: &str, state: &str) -> serde_json::Value {
    json!({ "name": name, "execution_id": id, "state": state, "exit_code": 0 })
}

#[test]
fn a_process_counts_when_it_is_the_only_one_with_its_name_and_is_running() -> Result<(), Box<dyn Error>> {
    let id = ExecutionId::new("x1");
    let running = listing(json!([process("svc", "x1", "running")]))?;
    assert!(only_running_process(&running, "svc", &id));
    Ok(())
}

#[test]
fn a_duplicate_a_stopped_process_or_another_id_does_not_count() -> Result<(), Box<dyn Error>> {
    let id = ExecutionId::new("x1");
    let twice = listing(json!([
        process("svc", "x1", "running"),
        process("svc", "x2", "running")
    ]))?;
    assert!(!only_running_process(&twice, "svc", &id));
    let exited = listing(json!([process("svc", "x1", "exited")]))?;
    assert!(!only_running_process(&exited, "svc", &id));
    let other = listing(json!([process("svc", "x2", "running")]))?;
    assert!(!only_running_process(&other, "svc", &id));
    Ok(())
}

#[test]
fn a_refused_listing_does_not_count() {
    let refused = ListProcessesResponse::Denied(trogon_sandbox_contract::LeaseError::Backend {
        message: "no".to_owned(),
    });
    assert!(!only_running_process(&refused, "svc", &ExecutionId::new("x1")));
}
