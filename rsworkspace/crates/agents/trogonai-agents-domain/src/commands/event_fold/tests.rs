use super::*;

#[test]
fn rejects_an_event_without_a_payload() {
    assert_eq!(
        provisioned_payload(&v1::AgentEvent::default()),
        Err(AgentEventFoldError::MissingEvent)
    );
}

#[test]
fn extracts_the_provisioned_payload() {
    let event = v1::AgentEvent {
        event: Some(v1::AgentProvisioned::default().into()),
    };

    assert!(provisioned_payload(&event).is_ok());
}

#[test]
fn rejects_a_mismatched_command_identity() {
    let expected = AgentId::parse("agent-1").unwrap();
    let actual = AgentId::parse("agent-2").unwrap();

    assert!(matches!(
        ensure_command_identity(&expected, &actual),
        Err(AgentEventFoldError::AgentIdMismatch { expected, actual })
            if expected == "agent-1" && actual.as_str() == "agent-2"
    ));
}
