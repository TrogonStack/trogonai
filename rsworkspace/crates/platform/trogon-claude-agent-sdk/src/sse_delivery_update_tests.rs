#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "The tests build the JSON values the transport parses.")
)]

use super::*;

fn update(text: &str) -> Option<SseDeliveryUpdate> {
    let value: Value = serde_json::from_str(text).ok()?;
    SseDeliveryUpdate::from_value(&value)
}

#[test]
fn accepts_string_fields_and_an_optional_timestamp() {
    assert_eq!(
        update(r#"{"event_id":"e","status":"s","extra":1}"#),
        Some(SseDeliveryUpdate {
            event_id: "e".to_owned(),
            status: "s".to_owned(),
            timestamp: None
        })
    );
    assert_eq!(
        update(r#"{"event_id":"e","status":"s","timestamp":"t"}"#).and_then(|update| update.timestamp),
        Some("t".to_owned())
    );
}

#[test]
fn rejects_anything_else() {
    assert_eq!(update("[]"), None);
    assert_eq!(update(r#"{"status":"s"}"#), None);
    assert_eq!(update(r#"{"event_id":"e"}"#), None);
    assert_eq!(update(r#"{"event_id":1,"status":"s"}"#), None);
    assert_eq!(update(r#"{"event_id":"e","status":"s","timestamp":null}"#), None);
    assert_eq!(update(r#"{"event_id":"e","status":2}"#), None);
}
