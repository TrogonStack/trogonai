use std::sync::Arc;

use trogon_sandbox_contract::TenantId;

use super::catalog_sweeper_open_error::CatalogSweeperOpenError;
use super::oci_artifact_lister::OciArtifactLister;
use super::snapshot_registry_inventory::SnapshotRegistryInventory;
use crate::snapshot_catalog::{ArtifactLister, SnapshotCatalog};

/// Opens the catalog sweeper over the NATS KV catalog. The orphan pass gets a
/// registry lister only when `orphan_sweep` is set; without it the sweeper
/// only trims to the keep-N bound in `config`.
pub async fn open_catalog_sweeper(
    js: async_nats::jetstream::Context,
    oci: &Arc<trogon_sandbox_oci::OciClient>,
    bus: &Arc<dyn crate::bus::BrokerBus>,
    registry: &str,
    tenants: &[TenantId],
    config: crate::snapshot_catalog::CatalogSweepConfig,
    orphan_sweep: bool,
) -> Result<Arc<crate::snapshot_catalog::CatalogSweeper>, CatalogSweeperOpenError> {
    let accounting = crate::snapshot_accounting::SnapshotAccountingStore::open(js.clone()).await?;
    let live = crate::snapshot_live::NatsKvLiveSnapshotStore::open(js.clone()).await?;
    let catalog: Arc<dyn SnapshotCatalog> = Arc::new(crate::snapshot_catalog::NatsKvSnapshotCatalog::open(js).await?);
    let lister = orphan_sweep.then(|| {
        Arc::new(OciArtifactLister::new(
            Arc::clone(oci) as Arc<dyn SnapshotRegistryInventory>,
            Arc::clone(&catalog),
            registry.to_string(),
            tenants.to_vec(),
        )) as Arc<dyn ArtifactLister>
    });
    Ok(Arc::new(
        crate::snapshot_catalog::CatalogSweeper::new(
            catalog,
            lister,
            Arc::clone(oci) as Arc<dyn crate::snapshot_catalog::ArtifactDeleter>,
            Arc::clone(bus),
            config,
        )
        .with_accounting(accounting, live),
    ))
}
