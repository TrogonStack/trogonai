//! Gateway-side AgentCard surfacing.
//!
//! The gateway's discover surface materializes responses from stored
//! AgentCard JSON. This module schema-validates the JSON via
//! `a2a-pack::accept_agent_card_on_read` before it's served — drift between
//! the stored payload and the AgentCard schema would otherwise be visible
//! to clients as opaque, unstructured JSON.

use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use serde_json::Value;

/// Validates AgentCard JSON before gateway/discover surfaces materialize a
/// response. Returns the same `Value` on accept (cloned so callers can take
/// ownership) and `None` when the schema check rejects it — callers turn
/// `None` into the appropriate "card unavailable" error for the surface.
#[must_use]
pub fn surface_agent_card_value(value: &Value) -> Option<Value> {
    accept_agent_card_on_read(value, AgentCardSource::GatewaySurface).then(|| value.clone())
}

#[cfg(test)]
mod tests;
