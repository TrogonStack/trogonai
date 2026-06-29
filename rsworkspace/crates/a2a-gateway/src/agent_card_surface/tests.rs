use serde_json::json;

use super::*;

#[test]
fn surfaces_valid_agent_card_json() {
    let card = json!({
        "name": "gw-agent",
        "supportedInterfaces": [{
            "url": "https://example.com/a2a",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "0.2.0"
        }]
    });
    assert_eq!(surface_agent_card_value(&card), Some(card));
}

#[test]
fn drops_empty_object_agent_card_json() {
    // An empty object has no required AgentCard fields; the schema check
    // refuses it so the gateway surface can return a typed "unavailable"
    // response instead of forwarding nonsense JSON.
    assert!(surface_agent_card_value(&json!({})).is_none());
}

#[test]
fn drops_agent_card_missing_supported_interfaces() {
    // `name` alone isn't a valid AgentCard — must be paired with at least
    // one `supportedInterfaces` entry.
    let card = json!({"name": "no-interfaces"});
    assert!(surface_agent_card_value(&card).is_none());
}

#[test]
fn drops_agent_card_with_garbage_in_interfaces() {
    let card = json!({
        "name": "gw-agent",
        "supportedInterfaces": "not-an-array"
    });
    assert!(surface_agent_card_value(&card).is_none());
}

#[test]
fn surfaced_value_round_trips_unchanged() {
    let card = json!({
        "name": "gw-agent",
        "supportedInterfaces": [{
            "url": "https://example.com/a2a",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "0.2.0"
        }]
    });
    let surfaced = surface_agent_card_value(&card).expect("accepted");
    // The contract is "validate-and-clone" — no mutation, no normalization.
    assert_eq!(surfaced, card);
}
