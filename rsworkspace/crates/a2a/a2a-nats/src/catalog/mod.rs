//! Catalog subsystem — agent card storage + import gating.
//!
//! Discover (federated SpiceDB-gated fan-out) lands in a follow-up PR after the
//! permission helper module ships; this slice keeps `list_cards_gated` behind
//! the generic [`ImportGate`] trait so the store doesn't depend on SpiceDB yet.

pub mod agent_view;
pub mod import_gate;
pub mod nats_kv;
pub mod registrar;
#[cfg(feature = "spicedb")]
pub mod spicedb_permission;
pub mod store;
pub mod watch;

pub use agent_view::{
    AgentViewCheckOutcome, AgentViewFilterError, AgentViewTuple, DiscoveryAgentFilterOutcome, DiscoveryHiddenReason,
    SpiceDbSessionKey, filter_agents_by_view,
};
pub use import_gate::{AllowAllImportGate, ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};
pub use registrar::RegistrarSubject;
#[cfg(feature = "spicedb")]
pub use spicedb_permission::{
    AgentViewGate, AgentViewGateLayer, ENV_TIER1_SPICEDB_ENABLED, ENV_TIER1_SPICEDB_ENDPOINT, ENV_TIER1_SPICEDB_TOKEN,
    ENV_TIER1_ZEDTOKEN_TTL_SECS, LiveAgentViewGate, NoopAgentViewGate, SpiceDbSessionCache, session_from_principal,
    spicedb_subject_from_principal as tier1_spicedb_subject_from_principal, tier1_enabled, tier1_zed_token_ttl,
};
pub use store::{CatalogStore, CatalogStoreError, KvCatalogStore};
pub use watch::{AgentCardWatchError, AgentCardWatchEvent, map_kv_entry};
