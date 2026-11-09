use trogon_claude_agent_sdk::{JsonObject, MessageOrigin, MessageOriginError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn message_origins_round_trip_future_kinds_and_extension_values() -> TestResult {
    let raw = r#"{"kind":"future-kind","nested":{"values":[null,true,7]},"source":"caller"}"#;
    let origin: MessageOrigin = serde_json::from_str(raw)?;
    assert_eq!(origin.kind(), "future-kind");
    assert_eq!(origin.additional_fields()["source"], "caller");
    assert!(origin.additional_fields().get("kind").is_none());
    assert_eq!(
        serde_json::to_value(origin.clone())?,
        serde_json::from_str::<serde_json::Value>(raw)?
    );
    let fields: JsonObject = serde_json::from_str(r#"{"source":"caller"}"#)?;
    let constructed = MessageOrigin::new("human", fields.clone())?;
    assert_eq!(constructed.kind(), "human");
    assert_eq!(constructed.additional_fields(), &fields);
    Ok(())
}

#[test]
fn message_origins_reject_invalid_kinds_and_reserved_extension_keys() -> TestResult {
    for kind in ["", "bad\nkind", "bad\0kind"] {
        assert!(matches!(
            MessageOrigin::new(kind, JsonObject::new()),
            Err(MessageOriginError::Kind)
        ));
    }
    let reserved = serde_json::from_str(r#"{"kind":"spoofed"}"#)?;
    assert!(matches!(
        MessageOrigin::new("human", reserved),
        Err(MessageOriginError::ReservedField)
    ));
    for raw in [
        r#"{}"#,
        r#"{"kind":""}"#,
        r#"{"kind":false}"#,
        r#"{"kind":"bad\nkind"}"#,
    ] {
        assert!(serde_json::from_str::<MessageOrigin>(raw).is_err());
    }
    assert_eq!(MessageOriginError::Kind.to_string(), "invalid message origin kind");
    assert_eq!(
        MessageOriginError::ReservedField.to_string(),
        "message origin extension fields contain a reserved kind field"
    );
    Ok(())
}
