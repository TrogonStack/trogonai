//! The `run_command` payloads the worker accepts and refuses at decode.

use serde_json::{Value, json};

use super::*;

fn payload(extra: Value) -> anyhow::Result<Vec<u8>> {
    let mut wire = json!({ "lease_id": "l", "command": ["cat"] });
    if let (Some(wire), Some(extra)) = (wire.as_object_mut(), extra.as_object()) {
        wire.extend(extra.clone());
    }
    Ok(serde_json::to_vec(&wire)?)
}

fn refusal(extra: Value) -> anyhow::Result<String> {
    match serde_json::from_slice::<RunCommandRequest>(&payload(extra)?) {
        Ok(_) => anyhow::bail!("the worker decoded a payload it must refuse"),
        Err(error) => Ok(error.to_string()),
    }
}

#[test]
fn run_command_spec_accepts_the_defaults() -> anyhow::Result<()> {
    let request: RunCommandRequest = serde_json::from_slice(&payload(json!({}))?)?;
    assert!(matches!(request.execution, CommandExecution::Attached));
    Ok(())
}

#[test]
fn run_command_spec_rejects_lease_deadline_without_detached() -> anyhow::Result<()> {
    let error = refusal(json!({ "deadline": "lease" }))?;
    assert!(
        error.contains("a lease deadline requires a detached command"),
        "{error}"
    );
    let request: RunCommandRequest =
        serde_json::from_slice(&payload(json!({ "deadline": "lease", "detached": true }))?)?;
    assert!(matches!(request.execution, CommandExecution::Detached { .. }));
    Ok(())
}

#[test]
fn run_command_spec_rejects_interactive_stdin_without_detached() -> anyhow::Result<()> {
    let error = refusal(json!({ "stdin": "interactive" }))?;
    assert!(
        error.contains("interactive stdin requires a detached command"),
        "{error}"
    );
    let request: RunCommandRequest =
        serde_json::from_slice(&payload(json!({ "stdin": "interactive", "detached": true }))?)?;
    assert!(matches!(request.execution, CommandExecution::Detached { .. }));
    Ok(())
}

#[test]
fn run_command_spec_rejects_the_reserved_execution_id_env_key() -> anyhow::Result<()> {
    let error = refusal(json!({ "env": { "SANDBOX_EXECUTION_ID": "forged" } }))?;
    assert!(
        error.contains("environment key SANDBOX_EXECUTION_ID is reserved"),
        "{error}"
    );
    Ok(())
}
