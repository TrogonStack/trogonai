//! First-party A2A policy bundle surface.
//!
//! Ships resource-tuple tables, catalog shaping, AgentCard schema, parts redaction,
//! audit envelope extensions, and rate-limit profiles for the gateway policy engine.
//! WASM substrate and SpiceDB wiring are out of scope for this skeleton crate.

pub mod agent_card_read;
pub mod agent_card_schema;
pub mod audit;
pub mod bundle;
pub mod catalog;
pub mod rate_limit;
pub mod redaction;
pub mod resource_tuples;
pub mod skills;

pub use agent_card_read::{
    AgentCardSchemaError, AgentCardSource, accept_agent_card_on_read, filter_agent_cards_on_read,
    validate_agent_card_on_read,
};
pub use agent_card_schema::{
    AGENT_CARD_JSON_SCHEMA, AgentCardJsonSchema, AgentCardValidateError, validate_agent_card_value,
};
pub use bundle::{PolicyBundle, VERSION};
