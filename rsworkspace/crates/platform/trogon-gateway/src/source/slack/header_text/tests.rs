use super::*;

#[test]
fn accepts_plain_text() {
    assert!(HeaderText::new("Ev01ABC123").is_ok());
    assert!(HeaderText::new("/trogon deploy").is_ok());
}

#[test]
fn rejects_line_feed() {
    assert_eq!(HeaderText::new("Ev01\nX-Injected: 1"), Err(InvalidHeaderText));
}

#[test]
fn rejects_carriage_return() {
    assert_eq!(HeaderText::new("Ev01\rX"), Err(InvalidHeaderText));
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
