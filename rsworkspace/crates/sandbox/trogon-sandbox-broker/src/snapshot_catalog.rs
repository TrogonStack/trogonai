//! Snapshot catalog: keep-N retention per source lease and an orphan sweep.
//!
//! The catalog is the set of [`trogon_sandbox_contract::SnapshotKvRecord`]s the
//! snapshot builder writes, one per published digest, under the snapshot KV
//! bucket. Each carries its tenant, source lease, registry location, parent and
//! creation time, which is all a retention pass needs. The tag-based retention
//! in `retention_sweep` prunes by what tags used to point at; this pass prunes
//! by how many snapshots one lease has accumulated, and finds artifacts the
//! catalog never heard of.
//!
//! # What is never pruned
//!
//! A digest is protected, and survives both passes, when any of these holds:
//!
//! - a lease that is still live, or that carries a name, was restored from it
//!   or sits on it as a remote dormancy artifact;
//! - a snapshot tag currently points at it;
//! - it is the parent of a protected digest (restore walks the lineage).
//!
//! Records without a creation time count toward the keep-N budget but are not
//! pruned, because their age is unknown.
//!
//! # Orphans
//!
//! The builder publishes the artifact before it writes the catalog record, so
//! an artifact with no record is normal for a moment. An orphan is only deleted
//! after it has been seen without a record on sweeps at least `orphan_grace`
//! apart. The registry has no listing call in the OCI client, so the artifacts
//! come from an [`ArtifactLister`]; without one the orphan pass does nothing.
mod artifact_delete_error;
mod artifact_deleter;
mod artifact_list_error;
mod artifact_lister;
mod catalog;
mod catalog_sweep_config;
mod catalog_sweep_report;
mod catalog_sweeper;
mod in_memory_snapshot_catalog;
mod listed_artifact;
mod nats_kv_snapshot_catalog;
mod orphan_tracker;
mod retention_planning;
mod snapshot_catalog_error;

pub use artifact_delete_error::ArtifactDeleteError;
pub use artifact_deleter::ArtifactDeleter;
pub use artifact_list_error::ArtifactListError;
pub use artifact_lister::ArtifactLister;
pub use catalog::SnapshotCatalog;
pub use catalog_sweep_config::CatalogSweepConfig;
pub use catalog_sweep_report::CatalogSweepReport;
pub use catalog_sweeper::CatalogSweeper;
pub use in_memory_snapshot_catalog::InMemorySnapshotCatalog;
pub use listed_artifact::ListedArtifact;
pub use nats_kv_snapshot_catalog::NatsKvSnapshotCatalog;
pub use orphan_tracker::OrphanTracker;
pub use retention_planning::{close_over_parents, find_orphans, lease_restore_sources, plan_keep_n};
pub use snapshot_catalog_error::SnapshotCatalogError;

#[cfg(test)]
mod tests;
