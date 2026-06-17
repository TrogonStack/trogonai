//! Per-account import allow/deny gate consulted before the catalog stamps an
//! agent card into a tenant's catalog KV.
//!
//! The SpiceDB-backed implementation lands in a follow-up PR; this slice ships
//! the trait, the error shape, and an `AllowAllImportGate` default that lets the
//! catalog work end-to-end without an authorisation backend wired up.

mod allow_all;
mod error;
mod gate;

pub mod principal;

#[cfg(test)]
mod tests;

pub use allow_all::AllowAllImportGate;
pub use error::ImportGateError;
pub use gate::ImportGate;
pub use principal::{ImportedAccountName, SpiceDbPrincipal};
