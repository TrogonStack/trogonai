use super::*;

#[test]
fn headers_preserve_ordered_pairs() {
    let headers = MessageHeaders::new([("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]).unwrap();

    assert_eq!(
        headers
            .as_slice()
            .iter()
            .map(|header| (header.name().as_str(), header.value().as_str()))
            .collect::<Vec<_>>(),
        vec![("x-kind", "heartbeat"), ("x-kind", "retry"), ("x-owner", "ops")]
    );
}

#[test]
fn invalid_header_name_is_rejected() {
    let error = MessageHeaders::new([("bad name", "value")]).unwrap_err();
    assert!(matches!(error, MessageHeadersError::InvalidName { .. }));
    assert_eq!(error.to_string(), "header name 'bad name' is invalid");
}

#[test]
fn message_conversion_uses_content_type_from_message_content() {
    let message = v1::Message::from(&MessageEnvelope {
        content: MessageContent::with_content_type("plain text", "text/plain"),
        headers: MessageHeaders::default(),
    });

    assert_eq!(message.content.as_option().unwrap().content_type, "text/plain");
}

#[test]
fn header_value_objects_cover_conversions_and_accessors() {
    let name = HeaderName::try_from("x-kind".to_string()).unwrap();
    let value = HeaderValue::try_from("heartbeat".to_string()).unwrap();
    let header = MessageHeader {
        name: name.clone(),
        value: value.clone(),
    };

    assert_eq!(name.as_str(), "x-kind");
    assert_eq!(name.into_string(), "x-kind");
    assert_eq!(value.as_str(), "heartbeat");
    assert_eq!(value.into_string(), "heartbeat");
    assert_eq!(
        header.into_pair(),
        (
            HeaderName::new("x-kind").unwrap(),
            HeaderValue::new("heartbeat").unwrap()
        )
    );
    assert!(HeaderName::try_from("bad name").is_err());
    assert!(HeaderValue::try_from("bad\nvalue").is_err());
}

#[test]
fn message_headers_cover_collection_helpers_and_errors() {
    let headers = MessageHeaders::try_from(vec![("x-kind".to_string(), "heartbeat".to_string())]).unwrap();
    let header_vec = headers.clone().into_vec();

    assert!(!headers.is_empty());
    assert_eq!(header_vec.len(), 1);
    assert_eq!(
        MessageHeadersError::InvalidName {
            name: "bad name".to_string()
        }
        .to_string(),
        "header name 'bad name' is invalid"
    );
    assert_eq!(
        MessageHeaders::new([("x-kind", "bad\rvalue")]).unwrap_err().to_string(),
        "header 'x-kind' contains an invalid value"
    );
}

#[test]
fn message_content_covers_constructors_and_byte_access() {
    let octets = MessageContent::from_static("raw");
    let plain = MessageContent::with_content_type("plain", "text/plain");
    let json = MessageContent::json("{}");
    let from_string = MessageContent::from("owned".to_string());
    let from_str = MessageContent::from("borrowed");

    assert_eq!(octets.content_type().as_str(), "application/octet-stream");
    assert_eq!(octets.as_ref(), b"raw");
    assert_eq!(plain.content_type().as_str(), "text/plain");
    assert_eq!(plain.as_str(), "plain");
    assert_eq!(json.content_type().as_str(), "application/json");
    assert_eq!(from_string.into_string(), "owned");
    assert_eq!(from_str.as_slice(), b"borrowed");
}

#[test]
fn message_conversion_includes_content_and_headers() {
    let message = v1::Message::from(&MessageEnvelope {
        content: MessageContent::json(r#"{"ok":true}"#),
        headers: MessageHeaders::new([("x-kind", "heartbeat")]).unwrap(),
    });

    let content = message.content.as_option().unwrap();
    assert_eq!(content.content_type, "application/json");
    assert_eq!(content.data, br#"{"ok":true}"#);
    assert_eq!(message.headers.len(), 1);
    assert_eq!(message.headers[0].name, "x-kind");
    assert_eq!(message.headers[0].value, "heartbeat");
}

mod prop_tests;
