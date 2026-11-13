use trogon_claude_agent_sdk::{JsonObject, SessionId, SystemEvent, SystemMessage};

#[test]
fn system_messages_keep_open_subtypes_metadata_and_optional_identity() -> Result<(), Box<dyn std::error::Error>> {
    let data: JsonObject = serde_json::from_str(r#"{"state":"requires_action","future":{"value":null}}"#)?;
    let message = SystemMessage {
        subtype: "session_state_changed".into(),
        session_id: None,
        data: data.clone(),
        event: SystemEvent::Other,
        raw: JsonObject::new(),
    };
    assert_eq!(message.subtype, "session_state_changed");
    assert!(message.session_id.is_none());
    assert_eq!(message.clone().data, data);
    let attributed = SystemMessage {
        subtype: "future_subtype".into(),
        session_id: Some(SessionId::from("session-one")),
        data,
        event: SystemEvent::Other,
        raw: JsonObject::new(),
    };
    assert_eq!(
        attributed.session_id.as_ref().map(SessionId::as_str),
        Some("session-one")
    );
    assert_eq!(attributed.subtype, "future_subtype");
    Ok(())
}
