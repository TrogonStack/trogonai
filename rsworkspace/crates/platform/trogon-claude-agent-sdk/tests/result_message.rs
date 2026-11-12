use trogon_claude_agent_sdk::{JsonObject, MessageId, MessageOrigin, ResultMessage, ResultStatus, SessionId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn result_messages_keep_optional_payloads_costs_identity_and_extensions() -> TestResult {
    let metadata: JsonObject = serde_json::from_str(r#"{"future":{"value":null}}"#)?;
    let result = ResultMessage {
        subtype: "error_during_execution".into(),
        status: ResultStatus::Error,
        duration_ms: 4,
        duration_api_ms: 2,
        num_turns: 1,
        session_id: SessionId::from("session-one"),
        total_cost_usd: Some(0.25),
        result: None,
        structured_output: Some(serde_json::Value::Null),
        usage: Some(metadata.clone()),
        errors: Some(vec!["cancelled".into()]),
        uuid: Some(MessageId::from("result-one")),
        stop_reason: None,
        terminal_reason: Some("aborted_streaming".into()),
        origin: Some(MessageOrigin::new("task-notification", JsonObject::new())?),
        model_usage: Some(metadata.clone()),
        permission_denials: Some(vec![serde_json::Value::Null]),
        deferred_tool_use: Some(metadata.clone()),
        api_error_status: Some(429),
        result_index: Some(0),
        queued_turn_count: Some(0),
        additional_fields: metadata.clone(),
    };
    let unreported = ResultMessage {
        errors: None,
        ..result.clone()
    };
    let unreported_errors = unreported.clone().errors;
    assert_eq!(unreported_errors, None);
    let reported_empty = ResultMessage {
        errors: Some(Vec::new()),
        ..result.clone()
    };
    let empty_errors = reported_empty.clone().errors;
    assert_eq!(empty_errors, Some(Vec::new()));
    assert_eq!(result.clone().status, ResultStatus::Error);
    assert_eq!(result.structured_output.as_ref(), Some(&serde_json::Value::Null));
    assert!(result.result.is_none());
    assert_eq!(result.total_cost_usd, Some(0.25));
    assert_eq!(result.session_id.as_str(), "session-one");
    assert_eq!(result.uuid.as_ref().map(MessageId::as_str), Some("result-one"));
    assert_eq!(
        result.origin.as_ref().map(MessageOrigin::kind),
        Some("task-notification")
    );
    assert_eq!(result.model_usage.as_ref(), Some(&metadata));
    assert_eq!(result.additional_fields, metadata);
    assert_eq!(result.api_error_status, Some(429));
    assert_eq!(result.result_index, Some(0));
    assert_eq!(result.queued_turn_count, Some(0));
    assert_eq!(result.errors, Some(vec!["cancelled".to_owned()]));
    Ok(())
}

#[test]
fn result_status_keeps_success_and_error_distinct() {
    assert_ne!(ResultStatus::Success, ResultStatus::Error);
}
