use std::sync::Arc;
use std::time::Duration;

use super::background_loops::BackgroundLoops;
use super::broker_services::BrokerServices;
use super::serve_error::ServeError;
use crate::artifact_lister::open_catalog_sweeper;
use crate::cli::Cli;
use crate::retention_sweep::{RetentionSweepConfig, run_retention_sweep};
use crate::snapshot_catalog::{CatalogSweepConfig, CatalogSweeper};

/// Starts the snapshot retention and eviction sweep.
pub(super) async fn start(cli: &Cli, services: &BrokerServices, loops: &BackgroundLoops) -> Result<(), ServeError> {
    let config = RetentionSweepConfig {
        tenants: cli.retention_sweep_tenants.clone(),
        registry: cli.snapshot_registry.clone(),
        discover_tenants: cli.retention_sweep_discover_tenants,
        active_leases: Some(Arc::clone(&services.leases.active_leases)),
        catalog: open_catalog(cli, services).await?,
        remover: Some(services.snapshot_remover(&cli.snapshot_registry)?),
    };
    loops.spawn(run_retention_sweep(
        Duration::from_millis(cli.retention_tick_ms),
        config,
        Arc::clone(&services.durable.tags),
        Arc::clone(&services.durable.snapshot_last_used),
        services.jetstream.clone(),
        Arc::clone(&services.oci),
        Arc::clone(&services.bus),
        loops.cancel(),
    ));
    Ok(())
}

/// The keep-N and orphan pass, opened only when either is turned on.
async fn open_catalog(cli: &Cli, services: &BrokerServices) -> Result<Option<Arc<CatalogSweeper>>, ServeError> {
    if cli.snapshot_keep_per_lease == 0 && !cli.snapshot_orphan_sweep {
        return Ok(None);
    }
    let sweeper = open_catalog_sweeper(
        services.jetstream.clone(),
        &services.oci,
        &services.bus,
        &cli.snapshot_registry,
        &cli.retention_sweep_tenants,
        CatalogSweepConfig {
            keep_per_lease: cli.snapshot_keep_per_lease,
            orphan_grace: Duration::from_secs(cli.snapshot_orphan_grace_secs),
        },
        cli.snapshot_orphan_sweep,
    )
    .await
    .map_err(ServeError::opening("snapshot catalog sweeper"))?;
    Ok(Some(sweeper))
}
