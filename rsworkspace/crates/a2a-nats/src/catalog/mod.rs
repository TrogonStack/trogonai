//! AgentCard catalog (KV-backed storage, `{prefix}.discover.*`, and `{prefix}.catalog.register.*`).
//!
//! For push-model catalog freshness via JetStream KV watches, see the repository file `docs/catalog-kv-watch.md`.

pub mod discover;
pub mod import_gate;
pub mod nats_kv;
pub mod registrar;
pub mod store;

pub use discover::{DiscoverService, DiscoverServiceError, DiscoverSubject};
pub use import_gate::{
    AllowAllImportGate, ImportGate, ImportGateError, ImportedAccountName, SpiceDbImportGate,
    SpiceDbImportGateBuildError, SpiceDbPrincipal, ENV_SPICEDB_ENDPOINT, ENV_SPICEDB_TOKEN,
    ENV_SPICEDB_ZEDTOKEN_TTL_SECS, resolve_import_gate,
};
pub use nats_kv::{A2A_AGENT_CARDS, catalog_bucket_config};
pub use registrar::{CatalogRegistrarService, CatalogRegistrarServiceError, RegistrarSubject};
pub use store::{CatalogStore, CatalogStoreError, KvCatalogStore};
