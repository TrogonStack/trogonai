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
    let constructed = MessageOrigin::new("human", fields.clone());
    assert_eq!(constructed.kind(), "human");
    assert_eq!(constructed.additional_fields(), &fields);
    for kind in ["", "bad\nkind", "bad\0kind", "bad\u{7f}kind"] {
        let preserved = MessageOrigin::new(kind, fields.clone());
        assert_eq!(preserved.kind(), kind);
        assert_eq!(preserved.additional_fields(), &fields);
        let wire = serde_json::to_value(preserved.clone())?;
        assert_eq!(wire["kind"], kind);
        let decoded: MessageOrigin = serde_json::from_value(wire)?;
        assert_eq!(decoded.kind(), kind);
        assert_eq!(decoded.additional_fields(), &fields);
    }
    Ok(())
}

#[test]
fn message_origins_reject_missing_nonstring_kinds_and_reserved_extension_keys() -> TestResult {
    let reserved: JsonObject = serde_json::from_str(r#"{"kind":"spoofed"}"#)?;
    assert!(matches!(
        MessageOrigin::validated("human", reserved.clone()),
        Err(MessageOriginError::ReservedField)
    ));
    let kept = MessageOrigin::new("human", reserved);
    assert_eq!(kept.kind(), "human");
    assert!(kept.additional_fields().is_empty());
    for raw in [
        r#"{}"#,
        r#"{"kind":false}"#,
        r#"{"kind":7}"#,
        r#"{"kind":null}"#,
        r#"{"kind":"human","kind":"peer"}"#,
    ] {
        assert!(serde_json::from_str::<MessageOrigin>(raw).is_err());
    }
    assert_eq!(
        MessageOriginError::ReservedField.to_string(),
        "message origin extension fields contain a reserved kind field"
    );
    Ok(())
}
