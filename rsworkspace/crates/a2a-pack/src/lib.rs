//! First-party A2A policy bundle surface — AgentCard read-side validation.
//!
//! Ships the JSON-Schema for the A2A AgentCard plus a read-time validator used by
//! every materialization path (KV store, federated import, discovery response,
//! gateway surface, and agent handler return values). Other policy primitives
//! (resource tuples, redaction, audit, signing, rate limits) live elsewhere.

pub mod agent_card_read;
pub mod agent_card_schema;

pub use agent_card_read::{
    AgentCardSchemaError, AgentCardSource, accept_agent_card_on_read, filter_agent_cards_on_read,
    validate_agent_card_on_read,
};
pub use agent_card_schema::{
    AGENT_CARD_JSON_SCHEMA, AgentCardJsonSchema, AgentCardValidateError, validate_agent_card_value,
};
