use super::*;

#[test]
fn headers_preserve_ordered_pairs() {
    let headers = MessageHeaders::new([("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]).unwrap();

    assert_eq!(
        headers.as_slice(),
        &[
            ("x-kind".to_string(), "heartbeat".to_string()),
            ("x-kind".to_string(), "retry".to_string()),
            ("x-owner".to_string(), "ops".to_string()),
        ]
    );
}

#[test]
fn invalid_header_name_is_rejected() {
    let error = MessageHeaders::new([("bad name", "value")]).unwrap_err();
    assert!(matches!(error, MessageHeadersError::InvalidName { .. }));
}

#[test]
fn header_name_with_colon_is_rejected() {
    let error = MessageHeaders::new([("x:kind", "value")]).unwrap_err();
    assert!(matches!(error, MessageHeadersError::InvalidName { .. }));
}

#[test]
fn invalid_header_value_is_rejected() {
    let error = MessageHeaders::new([("x-kind", "line1\nline2")]).unwrap_err();
    assert!(matches!(error, MessageHeadersError::InvalidValue { .. }));
}

#[test]
fn try_from_vec_validates() {
    let ok = MessageHeaders::try_from(vec![("x-kind".to_string(), "ok".to_string())]).unwrap();
    assert_eq!(ok.as_slice().len(), 1);
    assert!(MessageHeaders::try_from(vec![("bad name".to_string(), "v".to_string())]).is_err());
}

#[test]
fn from_pairs_skips_validation_and_round_trips_helpers() {
    let headers = MessageHeaders::from_pairs([("x-kind", "ok")]);
    assert!(!headers.is_empty());
    assert!(MessageHeaders::default().is_empty());
    assert_eq!(headers.into_vec(), vec![("x-kind".to_string(), "ok".to_string())]);
}

#[test]
fn headers_serde_round_trips_verbatim() {
    // `from_pairs` keeps a name a stricter validator would reject; deserialization
    // must read it back without re-validating.
    let headers = MessageHeaders::from_pairs([("bad name", "value")]);
    let json = serde_json::to_string(&headers).unwrap();
    let decoded: MessageHeaders = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.as_slice(), headers.as_slice());
}

#[test]
fn message_content_accessors() {
    let content = MessageContent::new("application/json", r#"{"k":1}"#);
    assert_eq!(content.content_type(), "application/json");
    assert_eq!(content.as_str(), r#"{"k":1}"#);
    assert_eq!(content.as_slice(), br#"{"k":1}"#);
    assert_eq!(AsRef::<[u8]>::as_ref(&content), br#"{"k":1}"#);
    assert_eq!(content.into_string(), r#"{"k":1}"#);
}

#[test]
fn message_envelope_serde_round_trips() {
    let envelope = MessageEnvelope {
        content: MessageContent::from_static("text/plain", "hello"),
        headers: MessageHeaders::from_pairs([("x-kind", "heartbeat")]),
    };
    let json = serde_json::to_string(&envelope).unwrap();
    let decoded: MessageEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, envelope);
}
