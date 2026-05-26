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
pub use audit::{
    AuditCallerVia, AuditEnvelopeExtensions, AuditPolicyDecision, AuditTaskLifecycleId, AuditTaskStateLabel,
    AuditTenantId,
};
pub use bundle::{PolicyBundle, VERSION};
pub use rate_limit::{
    MaxConcurrentStreamsPerCallerAgent, MaxUnaryRequestsPerMinute, RateLimitProfile, RateLimitProfileRow,
    RateLimitSkillKind, default_rate_limit_profiles, profile_for_skill_kind,
};
pub use resource_tuples::{
    Tier1A2aMethodSlug, Tier1DeriveError, Tier1Permission, Tier1ResourceId, Tier1ResourceTuple,
    Tier1ResourceTupleRow, Tier1ResourceTupleTable, Tier1ResourceType, Tier1TupleResourceShape,
};
