use super::*;

fn worker() -> anyhow::Result<WorkerId> {
    Ok(WorkerId::new("w1")?)
}

fn run_command(execution: serde_json::Value) -> anyhow::Result<RunCommandRequest> {
    let mut body = serde_json::json!({
        "lease_id": "l1",
        "command": ["cat"],
        "timeout_seconds": 5,
    });
    if let (Some(body), serde_json::Value::Object(extra)) = (body.as_object_mut(), execution) {
        body.extend(extra);
    }
    Ok(serde_json::from_value(body)?)
}

#[test]
fn the_heartbeat_reports_this_provider_and_its_live_capacity() -> anyhow::Result<()> {
    let heartbeat = build_heartbeat(&worker()?, 8, 3, crate::profile::PROFILE.clone(), Vec::new());
    assert_eq!(heartbeat.provider, ProviderKind::Firecracker);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 8);
    assert_eq!(heartbeat.capacity.current_leases, 3);
    assert!(heartbeat.cached_digests.is_empty());
    Ok(())
}

#[test]
fn verb_subjects_are_scoped_to_the_firecracker_provider() {
    assert_eq!(
        verb_subject("run_command"),
        "sandbox.provider.firecracker.run_command.>"
    );
    assert_eq!(
        verb_subject("write_stdin"),
        "sandbox.provider.firecracker.write_stdin.>"
    );
}

#[test]
fn detached_interactive_stdin_is_accepted() -> anyhow::Result<()> {
    let request = run_command(serde_json::json!({"detached": true, "stdin": "interactive"}))?;
    assert_eq!(run_command_spec_violation(&request), None);
    Ok(())
}

#[test]
fn lease_deadline_is_still_refused() -> anyhow::Result<()> {
    let request = run_command(serde_json::json!({"detached": true, "deadline": "lease"}))?;
    assert!(run_command_spec_violation(&request).is_some());
    Ok(())
}

#[test]
fn required_launch_cancellation_is_refused() -> anyhow::Result<()> {
    let request = run_command(serde_json::json!({"require_launch_cancellation": true}))?;
    assert!(run_command_spec_violation(&request).is_some());
    Ok(())
}

#[test]
fn the_profile_advertises_interactive_stdin() {
    assert!(
        crate::profile::PROFILE
            .isolation
            .supports_interactive_stdin
            .is_supported()
    );
}
