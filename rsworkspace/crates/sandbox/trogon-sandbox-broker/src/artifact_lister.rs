//! Lists snapshot artifacts from an OCI registry for the orphan sweep.
//!
//! Every snapshot push leaves a staging tag, `sandbox-artifact-sha256-<hex>`,
//! next to the digest it pushed. The lister reads the distribution tag list of
//! each snapshot repository it knows and turns those tags back into digests, so
//! it needs no registry catalog endpoint, which many registries do not offer.
//!
//! # Which repositories
//!
//! The repositories named by catalog records, plus the current and legacy
//! repository of every catalog tenant and every configured tenant. A
//! repository that no record and no configured tenant points to is not seen,
//! so an artifact in a tenant's repository is only found once the tenant owns
//! at least one catalog record or is listed in the configuration.
//!
//! # What counts as an artifact
//!
//! Only manifests whose artifact type is a snapshot type. Referrers such as an
//! SBOM or a scan report share the staging tag scheme and have no catalog
mod catalog_sweeper_open_error;
mod oci_artifact_lister;
mod open_catalog_sweeper;
mod registry_inventory_error;
mod snapshot_registry_inventory;

pub use catalog_sweeper_open_error::CatalogSweeperOpenError;
pub use oci_artifact_lister::OciArtifactLister;
pub use open_catalog_sweeper::open_catalog_sweeper;
pub use registry_inventory_error::RegistryInventoryError;
pub use snapshot_registry_inventory::SnapshotRegistryInventory;

#[cfg(test)]
mod tests;
