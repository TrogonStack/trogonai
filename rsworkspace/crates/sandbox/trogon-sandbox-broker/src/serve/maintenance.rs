use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::background_loops::BackgroundLoops;
use super::broker_services::BrokerServices;
use super::constants::{OPERATIONS_DRIVE_INTERVAL, PUBLICATION_RECOVERY_INTERVAL};
use super::retention_setup;
use super::serve_error::ServeError;
use crate::cli::Cli;
use crate::core::BrokerCore;
use crate::entity_store::SandboxEntityMode;
use crate::idle::IdlePauser;
use crate::prewarm::{NatsKvPrewarmStore, PREWARM_TICK, PrewarmController, run_prewarm_controller};
use crate::sweeper::run_sweeper;

/// Starts every loop that keeps the broker's records consistent between requests.
pub(super) async fn start(
    cli: &Cli,
    services: &BrokerServices,
    broker: &Arc<BrokerCore>,
    loops: &BackgroundLoops,
) -> Result<(), ServeError> {
    recover_publications(services, loops);
    if cli.entity_mode != SandboxEntityMode::Off {
        Arc::clone(broker).spawn_entity_sweeper(
            Duration::from_millis(cli.entity_sweep_tick_ms),
            cli.entity_reconcile,
            loops.cancel(),
        );
    }
    start_prewarm(cli, services, broker, loops).await?;
    let tick = Duration::from_millis(cli.reclamation_tick_ms);
    let reconciled = Arc::clone(broker);
    loops.every(tick, move || reconcile(Arc::clone(&reconciled)));
    let driven = Arc::clone(broker);
    loops.every(OPERATIONS_DRIVE_INTERVAL, move || drive_operations(Arc::clone(&driven)));
    let pauser: Arc<dyn IdlePauser> = Arc::clone(broker) as _;
    loops.spawn(run_sweeper(
        tick,
        Arc::clone(&services.leases.active_leases),
        Arc::clone(&services.leases.quota),
        Arc::clone(&services.bus),
        services.metrics.clone(),
        Arc::clone(&services.durable.usage),
        pauser,
        loops.cancel(),
    ));
    retention_setup::start(cli, services, loops).await
}

/// Retries snapshot publications whose accounting outbox entry is still pending.
fn recover_publications(services: &BrokerServices, loops: &BackgroundLoops) {
    let Some(accounting) = services.leases.quota.snapshot_store() else {
        return;
    };
    loops.every(PUBLICATION_RECOVERY_INTERVAL, move || {
        let accounting = Arc::clone(&accounting);
        async move {
            if let Err(reason) = accounting.recover_publications().await {
                warn!(?reason, "snapshot publication outbox recovery remains pending");
            }
        }
    });
}

async fn start_prewarm(
    cli: &Cli,
    services: &BrokerServices,
    broker: &Arc<BrokerCore>,
    loops: &BackgroundLoops,
) -> Result<(), ServeError> {
    let store = NatsKvPrewarmStore::new(services.jetstream.clone())
        .await
        .map_err(ServeError::opening("prewarm store"))?;
    let controller = Arc::new(PrewarmController::new(
        Arc::clone(broker),
        Arc::new(store),
        Arc::clone(&services.live_snapshots),
        Arc::clone(&services.worker_registry),
        cli.snapshot_registry.clone(),
    ));
    loops.spawn(run_prewarm_controller(controller, PREWARM_TICK, loops.cancel()));
    info!("prewarm controller started");
    Ok(())
}

async fn reconcile(broker: Arc<BrokerCore>) {
    broker.reconcile_expired_leases().await;
    broker.reconcile_stalled_freezes().await;
    broker.reconcile_pending_purges().await;
    broker.reconcile_pending_releases().await;
    broker.reconcile_orphan_drive_claims().await;
}

async fn drive_operations(broker: Arc<BrokerCore>) {
    broker.drive_pending_operations().await;
}
