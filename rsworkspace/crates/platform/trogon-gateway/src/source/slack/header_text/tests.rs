use super::*;

#[test]
fn accepts_plain_text() {
    assert!(HeaderText::new("Ev01ABC123").is_ok());
    assert!(HeaderText::new("/trogon deploy").is_ok());
}

#[test]
fn rejects_line_feed() {
    assert_eq!(HeaderText::new("Ev01\nX-Injected: 1"), Err(InvalidHeaderTextError));
}

#[test]
fn rejects_carriage_return() {
    assert_eq!(HeaderText::new("Ev01\rX"), Err(InvalidHeaderTextError));
}

#[test]
fn or_unknown_keeps_valid_text_and_replaces_unsafe_text() {
    let headers = DeliveryHeaders::new()
        .with("Valid", &HeaderText::or_unknown("T01ABC"))
        .with("Unsafe", &HeaderText::or_unknown("T01\r\nX: 1"))
        .into_inner();

    assert_eq!(headers.get("Valid").map(|v| v.as_str()), Some("T01ABC"));
    assert_eq!(headers.get("Unsafe").map(|v| v.as_str()), Some("unknown"));
}

#[test]
fn digest_renders_the_prefix_and_lowercase_hex() {
    let headers = DeliveryHeaders::new()
        .with("Digest", &HeaderText::digest("id-", &[0x0a, 0xff]))
        .into_inner();

    assert_eq!(headers.get("Digest").map(|v| v.as_str()), Some("id-0aff"));
}

#[test]
fn delivery_headers_carry_validated_and_static_values() {
    let headers = DeliveryHeaders::new()
        .with("X-Slack-Team-Id", &HeaderText::new("T01ABC").unwrap())
        .with_static("X-Slack-Payload-Kind", "event")
        .into_inner();

    assert_eq!(headers.get("X-Slack-Team-Id").map(|v| v.as_str()), Some("T01ABC"));
    assert_eq!(headers.get("X-Slack-Payload-Kind").map(|v| v.as_str()), Some("event"));
}
