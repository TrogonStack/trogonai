use trogon_claude_agent_sdk::{
    AssistantMessage, Content, JsonObject, Message, ResultMessage, ResultStatus, SessionId, StreamEvent, SystemEvent,
    SystemMessage, UserMessage,
};

#[test]
fn message_envelopes_keep_declared_variants_and_boxed_results() -> Result<(), Box<dyn std::error::Error>> {
    let session = SessionId::from("session-one");
    let result = ResultMessage {
        subtype: "success".into(),
        status: ResultStatus::Success,
        duration_ms: 1,
        duration_api_ms: 1,
        num_turns: 1,
        session_id: session.clone(),
        total_cost_usd: None,
        result: Some("done".into()),
        structured_output: None,
        usage: None,
        errors: Some(vec![]),
        uuid: None,
        stop_reason: None,
        terminal_reason: None,
        origin: None,
        model_usage: None,
        permission_denials: None,
        deferred_tool_use: None,
        api_error_status: None,
        result_index: None,
        queued_turn_count: None,
        additional_fields: JsonObject::new(),
        raw: JsonObject::new(),
    };
    let variants = [
        Message::Assistant(AssistantMessage {
            content: vec![],
            model: "model".into(),
            parent_tool_use_id: None,
            session_id: None,
            error: None,
            usage: None,
            uuid: None,
            message_id: None,
            stop_reason: None,
            additional_fields: JsonObject::new(),
            message_fields: JsonObject::new(),
            raw: JsonObject::new(),
        }),
        Message::User(UserMessage {
            content: Content::Text("prompt".into()),
            parent_tool_use_id: None,
            session_id: None,
            uuid: None,
            origin: None,
            tool_use_result: None,
            additional_fields: JsonObject::new(),
            message_fields: JsonObject::new(),
            raw: JsonObject::new(),
        }),
        Message::System(SystemMessage {
            subtype: "future".into(),
            session_id: None,
            data: JsonObject::new(),
            event: SystemEvent::Other,
            raw: JsonObject::new(),
        }),
        Message::Result(Box::new(result)),
        Message::StreamEvent(StreamEvent {
            uuid: "stream-one".into(),
            session_id: session,
            event: JsonObject::new(),
            additional_fields: JsonObject::new(),
            parent_tool_use_id: None,
            raw: JsonObject::new(),
        }),
        Message::Unknown {
            kind: "future_message".into(),
            data: serde_json::from_str(r#"{"future":null}"#)?,
            raw: serde_json::from_str(r#"{"type":"future_message","future":null}"#)?,
        },
    ];
    let names: Vec<_> = variants
        .iter()
        .map(|message| match message {
            Message::Assistant(_) => "assistant",
            Message::User(_) => "user",
            Message::System(_) => "system",
            Message::Result(result) => {
                let value: &ResultMessage = result.as_ref();
                assert_eq!(value.result.as_deref(), Some("done"));
                "result"
            }
            Message::StreamEvent(_) => "stream_event",
            Message::RateLimit(_) => "rate_limit_event",
            Message::ConversationReset(_) => "conversation_reset",
            Message::Unknown { kind, data, raw } => {
                assert_eq!(kind, "future_message");
                assert_eq!(data.get("future"), Some(&serde_json::Value::Null));
                assert_eq!(
                    raw.get("type").and_then(serde_json::Value::as_str),
                    Some("future_message")
                );
                "unknown"
            }
        })
        .collect();
    assert_eq!(
        names,
        ["assistant", "user", "system", "result", "stream_event", "unknown"]
    );
    assert!(matches!(variants[3].clone(), Message::Result(result) if result.status == ResultStatus::Success));
    Ok(())
}
