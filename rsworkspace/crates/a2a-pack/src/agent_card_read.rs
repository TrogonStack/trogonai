//! Read-side AgentCard JSON-Schema enforcement for non-KV materialization paths.

use serde_json::Value;
use tracing::warn;

use crate::agent_card_schema::{AgentCardValidateError, validate_agent_card_value};

/// Telemetry label for where an AgentCard payload was materialized before read validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentCardSource {
    KvStore,
    FederatedImport,
    DiscoverResponse,
    GatewaySurface,
    AgentHandler,
}

impl AgentCardSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KvStore => "kv_store",
            Self::FederatedImport => "federated_import",
            Self::DiscoverResponse => "discover_response",
            Self::GatewaySurface => "gateway_surface",
            Self::AgentHandler => "agent_handler",
        }
    }
}

/// Alias aligned with catalog/gateway call sites.
pub type AgentCardSchemaError = AgentCardValidateError;

/// Validates an AgentCard document on read, tagging the materialization source for callers.
pub fn validate_agent_card_on_read(
    value: &Value,
    source: AgentCardSource,
) -> Result<(), AgentCardSchemaError> {
    validate_agent_card_value(value).map_err(|error| {
        warn!(
            source = source.as_str(),
            error = %error,
            "AgentCard read validation failed; rejecting card"
        );
        error
    })
}

/// Returns `true` when the card passes read validation (logs and rejects on failure).
#[must_use]
pub fn accept_agent_card_on_read(value: &Value, source: AgentCardSource) -> bool {
    validate_agent_card_on_read(value, source).is_ok()
}

/// Drops schema-violating cards; retains valid entries in input order.
#[must_use]
pub fn filter_agent_cards_on_read(values: Vec<Value>, source: AgentCardSource) -> Vec<Value> {
    values
        .into_iter()
        .filter(|value| accept_agent_card_on_read(value, source))
        .collect()
}

#[cfg(test)]
mod tests {
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
        assert!(!accept_agent_card_on_read(&json!({}), AgentCardSource::DiscoverResponse));
    }

    #[test]
    fn filter_drops_invalid_and_keeps_valid_batch() {
        let good = minimal_valid();
        let bad = json!({ "name": "" });
        let filtered = filter_agent_cards_on_read(
            vec![good.clone(), bad, good.clone()],
            AgentCardSource::FederatedImport,
        );
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0], good);
    }
}
