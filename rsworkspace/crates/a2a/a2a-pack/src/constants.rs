//! Crate-wide constants for a2a-pack.

/// Bundled draft-07 JSON Schema for minimal AgentCard registration checks.
pub const AGENT_CARD_JSON_SCHEMA: &str = include_str!("../schemas/agent-card.min.json");
