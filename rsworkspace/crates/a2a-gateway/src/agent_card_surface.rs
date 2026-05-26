use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use serde_json::Value;

/// Validates AgentCard JSON before gateway/discover surfaces materialize a response.
#[must_use]
pub fn surface_agent_card_value(value: &Value) -> Option<Value> {
    accept_agent_card_on_read(value, AgentCardSource::GatewaySurface).then(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn drops_invalid_agent_card_json() {
        assert!(surface_agent_card_value(&json!({})).is_none());
    }
}
