use super::*;
use serde_json::json;

fn minimal_valid() -> Value {
    json!({
        "name": "my-agent",
        "supportedInterfaces": [{
            "url": "https://example.com/a2a",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "0.2.0"
        }]
    })
}

#[test]
fn accepts_minimal_valid_json() {
    validate_agent_card_value(&minimal_valid()).expect("minimal card should validate");
}

#[test]
fn rejects_empty_object() {
    let err = validate_agent_card_value(&json!({})).unwrap_err();
    assert!(err.details().iter().any(|detail| detail.contains("name")));
    assert!(!err.to_string().is_empty());
}

#[test]
fn rejects_empty_name() {
    let mut card = minimal_valid();
    card["name"] = json!("");
    let err = validate_agent_card_value(&card).unwrap_err();
    assert!(err.details().iter().any(|detail| detail.contains("name")));
}

#[test]
fn rejects_missing_primary_interface_protocol_version() {
    let card = json!({
        "name": "my-agent",
        "supportedInterfaces": [{
            "url": "https://example.com/a2a",
            "protocolBinding": "JSONRPC"
        }]
    });
    let err = validate_agent_card_value(&card).unwrap_err();
    assert!(err.details().iter().any(|detail| detail.contains("protocolVersion")));
}

#[test]
fn rejects_missing_supported_interfaces_array() {
    let card = json!({
        "name": "my-agent",
    });
    let err = validate_agent_card_value(&card).unwrap_err();
    assert!(
        err.details()
            .iter()
            .any(|detail| detail.contains("supportedInterfaces"))
    );
}

#[test]
fn rejects_non_http_interface_url() {
    let mut card = minimal_valid();
    card["supportedInterfaces"][0]["url"] = json!("nats://example/a2a");
    let err = validate_agent_card_value(&card).unwrap_err();
    assert!(err.details().iter().any(|detail| detail.contains("url")));
}
