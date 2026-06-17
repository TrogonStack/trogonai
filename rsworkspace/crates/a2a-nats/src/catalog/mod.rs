//! Catalog subsystem — agent card storage + import gating.
//!
//! Discover (federated SpiceDB-gated fan-out) lands in a follow-up PR after the
//! permission helper module ships; this slice keeps `list_cards_gated` behind
//! the generic [`ImportGate`] trait so the store doesn't depend on SpiceDB yet.

pub mod import_gate;
pub mod nats_kv;
pub mod registrar;
pub mod store;
pub mod watch;

pub use import_gate::{AllowAllImportGate, ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};
pub use registrar::RegistrarSubject;
pub use store::{CatalogStore, CatalogStoreError, KvCatalogStore};
pub use watch::{AgentCardWatchError, AgentCardWatchEvent, map_kv_entry};
