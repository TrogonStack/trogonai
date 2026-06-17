//! Per-account import allow/deny gate consulted before the catalog stamps an
//! agent card into a tenant's catalog KV.
//!
//! The SpiceDB-backed implementation is gated behind the `spicedb` Cargo
//! feature so deployments without an authorisation backend don't pay the
//! authzed/tonic/moka compile cost; this slice always ships the trait, the
//! error shape, and an `AllowAllImportGate` default.

mod allow_all;
mod error;
mod gate;
#[cfg(feature = "spicedb")]
mod spicedb;

pub mod principal;

#[cfg(test)]
mod tests;

pub use allow_all::AllowAllImportGate;
pub use error::ImportGateError;
pub use gate::ImportGate;
pub use principal::{ImportedAccountName, SpiceDbPrincipal};
#[cfg(feature = "spicedb")]
pub use spicedb::{
    BulkImportPermissionCheck, ENV_SPICEDB_ENDPOINT, ENV_SPICEDB_TOKEN, ENV_SPICEDB_ZEDTOKEN_TTL_SECS,
    LiveBulkImportPermissionClient, SpiceDbEndpoint, SpiceDbImportGate, SpiceDbImportGateBuildError, SpiceDbToken,
    ZedTokenSnapshot, ZedTokenTtl, parse_subject_reference, resolve_import_gate, spicedb_subject_from_principal,
};
