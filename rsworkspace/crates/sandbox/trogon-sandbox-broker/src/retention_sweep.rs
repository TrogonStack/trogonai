//! Periodic sweeper that enforces `RetentionPolicy` on named/auto-tagged snapshots.
//!
//! # Tenants
//!
//! The sweeper considers the tenants in `RetentionSweepConfig::tenants`. With
//! `discover_tenants` it also adds every tenant that owns a tag, read from the
//! tenant-scoped keys of the tag store (`SnapshotTagStore::tenants`), so no
//! fixed list is needed. Discovery is off by default because it widens the
//! sweep from the listed tenants to all of them.
//!
//! # Protected digests
//!
//! A digest that a live or named lease was restored from, or that rests as its
//! remote dormancy artifact, is never evicted, whatever its tag history says.
//! When the lease set cannot be read the tick is skipped, so an unreadable set
//! is never mistaken for an empty one. The keep-N and orphan passes of
//! `snapshot_catalog` run after the tag pass with the same protection.

mod retention_sweep_config;
mod run_retention_sweep;

pub use retention_sweep_config::RetentionSweepConfig;
pub use run_retention_sweep::run_retention_sweep;
#[cfg(test)]
pub(crate) use run_retention_sweep::{
    apply_last_used_resets, evict_snapshot, live_restore_sources, resolve_tenants, select_eviction_candidates,
    sweep_once,
};

#[cfg(test)]
mod tests;
