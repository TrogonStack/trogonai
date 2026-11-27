//! Finds which per-tenant repository holds a snapshot.
//!
//! New snapshots go to
//! [`trogon_sandbox_protocol::snapshot_repository_for_tenant`].
//! For tenants whose id contains `:` or `/` that differs from the legacy
//! repository their older snapshots were pushed to, and nothing records
//! which of the two a given digest lives in, so restores and deletes ask the
//! registry: the current repository first, then the legacy one. Every other
//! tenant has one repository and is never probed.

mod locate_snapshot_repository;
mod snapshot_manifest_probe;

pub use locate_snapshot_repository::locate_snapshot_repository;
pub use snapshot_manifest_probe::SnapshotManifestProbe;

#[cfg(test)]
mod tests;
