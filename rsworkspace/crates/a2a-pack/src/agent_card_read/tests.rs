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
fn valid_card_passes_each_source() {
    let card = minimal_valid();
    for source in [
        AgentCardSource::FederatedImport,
        AgentCardSource::DiscoverResponse,
        AgentCardSource::GatewaySurface,
        AgentCardSource::AgentHandler,
    ] {
        assert!(accept_agent_card_on_read(&card, source));
    }
}

#[test]
fn schema_violating_card_is_rejected() {
    assert!(!accept_agent_card_on_read(
        &json!({}),
        AgentCardSource::DiscoverResponse
    ));
}

#[test]
fn filter_drops_invalid_and_keeps_valid_batch() {
    let good = minimal_valid();
    let bad = json!({ "name": "" });
    let filtered =
        filter_agent_cards_on_read(vec![good.clone(), bad, good.clone()], AgentCardSource::FederatedImport);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0], good);
}

#[test]
fn agent_card_source_as_str_covers_every_variant() {
    assert_eq!(AgentCardSource::KvStore.as_str(), "kv_store");
    assert_eq!(AgentCardSource::FederatedImport.as_str(), "federated_import");
    assert_eq!(AgentCardSource::DiscoverResponse.as_str(), "discover_response");
    assert_eq!(AgentCardSource::GatewaySurface.as_str(), "gateway_surface");
    assert_eq!(AgentCardSource::AgentHandler.as_str(), "agent_handler");
}

#[test]
fn validate_on_read_returns_error_for_invalid_card() {
    let err = validate_agent_card_on_read(&json!({}), AgentCardSource::KvStore).unwrap_err();
    assert!(!err.to_string().is_empty());
}
