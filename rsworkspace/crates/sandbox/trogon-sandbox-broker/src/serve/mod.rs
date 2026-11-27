//! Broker service wiring: opens the stores, starts the background loops and serves every subject
//! until shutdown.

mod background_loops;
mod broker_assembly;
mod broker_extensions;
mod broker_services;
mod constants;
mod durable_stores;
mod entity_shadow_setup;
mod event_routes;
mod handlers;
mod lease_route;
mod lease_routes;
mod lease_stores;
mod maintenance;
mod metrics_endpoint;
mod on_decode_error;
mod reply;
mod retention_setup;
mod route_context;
mod router;
mod serve_error;
mod startup_warnings;
mod tenant_policies;
mod tenant_route;
mod tenant_routes;
mod worker_registry_setup;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{info, warn};
use trogon_std::env::ReadEnv;

use self::background_loops::BackgroundLoops;
use self::broker_services::BrokerServices;
use self::route_context::RouteContext;
use self::router::Router;
use crate::cli::Cli;
use crate::digest_pinning::run_digest_pin_watcher;
use crate::metrics::BrokerMetrics;
use crate::quota::SnapshotQuota;

pub use constants::SERVICE_NAME;
pub use serve_error::ServeError;

/// Runs the broker until `shutdown` fires, then stops taking messages, waits for the replies and
/// background work already under way, and returns.
///
/// Settings that need a secret or key file, such as the dispatch signer, are read from `env`.
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(unstructured_log_fields, reason = "the log line is a single operator message")
)]
pub async fn run(cli: Cli, env: &impl ReadEnv, shutdown: CancellationToken) -> Result<(), ServeError> {
    let mut cli = cli;
    if cli.apply_insecure_dev() {
        warn!("{}", constants::INSECURE_DEV_WARNING);
    }
    let background = shutdown.child_token();
    let _stop_on_error = background.clone().drop_guard();
    let tasks = TaskTracker::new();
    let loops = BackgroundLoops::new(tasks.clone(), background.clone());

    let metrics = BrokerMetrics::new()?;
    metrics_endpoint::spawn(
        &loops,
        cli.observability
            .resolve_metrics_addr(trogon_sandbox_observability::BROKER),
        metrics.registry().clone(),
    );
    let nats = trogon_sandbox_protocol::nats_connect::connect(&cli.nats_url).await?;
    info!(nats_url = %cli.nats_url, "broker starting");
    if let Some(path) = &cli.cosign_public_key {
        info!(
            cosign_public_key = %path.display(),
            "Cosign verification key configured (workers must use the same PEM path)"
        );
    }

    let services = BrokerServices::open(&cli, env, nats, metrics, background.clone()).await?;
    startup_warnings::log(&cli);
    let auth = crate::auth_setup::connect(&cli, services.nats.clone()).await?;
    let broker = broker_assembly::assemble(&cli, &services)?;
    let broker = Arc::new(broker_extensions::attach(broker, &cli, env, &services.jetstream).await?);
    broker.bind_self();
    let recovered = broker.recover_operations().await;
    if recovered > 0 {
        info!(recovered, "re-drove unfinished operations");
    }
    maintenance::start(&cli, &services, &broker, &loops).await?;

    let context = Arc::new(RouteContext {
        nats: services.nats.clone(),
        broker,
        auth,
        lease_tenants: Arc::default(),
        live_snapshots: Arc::clone(&services.live_snapshots),
        quota: Arc::clone(&services.leases.quota),
        default_snapshot_quota: SnapshotQuota::new(cli.default_snapshot_count_cap, cli.default_snapshot_bytes_cap),
        active_leases: Arc::clone(&services.leases.active_leases),
    });
    let router = Router::new(Arc::clone(&context), tasks.clone(), background.clone());
    tenant_routes::register(&router).await?;
    lease_routes::register(&router).await?;
    event_routes::register(&router).await?;
    watch_digest_pins(&services, &context, &loops);

    shutdown.cancelled().await;
    background.cancel();
    tasks.close();
    tasks.wait().await;
    Ok(())
}

/// Checks the digest every restored snapshot reports against the one the broker pinned for it.
fn watch_digest_pins(services: &BrokerServices, context: &RouteContext, loops: &BackgroundLoops) {
    let watcher = run_digest_pin_watcher(
        services.nats.clone(),
        Arc::clone(&services.snapshot_pins),
        Arc::clone(&services.bus),
        services.metrics.clone(),
        Arc::clone(&context.lease_tenants),
        loops.cancel(),
    );
    loops.spawn(async move {
        if let Err(error) = watcher.await {
            warn!(error = %error, "digest pin watcher exited");
        }
    });
}

#[cfg(test)]
mod tests;
