use super::*;

#[test]
fn hook_output_uses_wire_names_and_typed_decisions() -> Result<(), serde_json::Error> {
    let output = HookOutput::Sync(Box::new(SyncHookOutput {
        continuation: Some(HookContinuation::Stop),
        specific: Some(HookSpecificOutput::PreToolUse {
            permission_decision: Some(HookPermissionDecision::Deny),
            permission_decision_reason: Some("policy".into()),
            updated_input: None,
            additional_context: None,
        }),
        ..SyncHookOutput::default()
    }))
    .to_wire_json()?;
    assert_eq!(output["continue"], false);
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(output.get("continue_").is_none());
    Ok(())
}

#[test]
fn matcher_seconds_and_async_milliseconds_are_distinct() -> Result<(), Box<dyn std::error::Error>> {
    let timeout = Timeout::new(std::time::Duration::from_millis(1500));
    let registration = HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: None,
        callbacks: Vec::new(),
        timeout: Some(timeout),
        on_failure: Some(HookFailureMode::Block),
    };
    assert_eq!(registration.to_wire_json(&[])?["timeout"], 1.5);
    assert_eq!(
        HookOutput::Async { timeout: Some(timeout) }.to_wire_json()?["asyncTimeout"],
        1500
    );
    Ok(())
}
