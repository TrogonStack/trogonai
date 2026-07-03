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
pub fn validate_agent_card_on_read(value: &Value, source: AgentCardSource) -> Result<(), AgentCardSchemaError> {
    let source_label = source.as_str();
    validate_agent_card_value(value).inspect_err(|error| {
        warn!(source = source_label, %error, "AgentCard read validation failed; rejecting card");
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
mod tests;
