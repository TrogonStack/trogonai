use std::sync::Arc;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_std::env::ReadEnv;

use crate::actor_index::NatsActorIndex;
use crate::adapter::SubstrateProvider;
use crate::ate_client::TonicControl;
use crate::recovery::{self, RecoveryError, RecoveryReport};
use crate::template::{NatsTemplateIndex, TemplateRegistry};

use super::constants::CONNECT_TIMEOUT;
use super::{ServeError, WorkerArgs, run, spawn_heartbeat_task};

/// Connects to NATS and the control plane, recovers what a previous run left behind, and serves
/// leases until `cancel` fires.
pub async fn serve(args: &WorkerArgs, env: &impl ReadEnv, cancel: CancellationToken) -> Result<(), ServeError> {
    let config = args.substrate_config()?;
    let security = args.connect_security()?;

    let nats = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| ServeError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let js = jetstream::new(nats.clone());

    let control = TonicControl::connect(&args.ate_api_endpoint, CONNECT_TIMEOUT, security)
        .await
        .map_err(|source| ServeError::Control {
            endpoint: args.ate_api_endpoint.clone(),
            source,
        })?;

    let templates = TemplateRegistry::new(Arc::new(NatsTemplateIndex::new(js.clone()).await?));
    let index = Arc::new(NatsActorIndex::new(js.clone()).await?);

    let provider =
        SubstrateProvider::new(config, Arc::new(control), templates, index, env)?.with_lifecycle_stream(js.clone());
    let provider = Arc::new(match &args.worker_id {
        Some(worker_id) => provider.with_worker_id(worker_id.clone()),
        None => provider,
    });
    let worker_id = provider.worker_id().clone();

    // Before the first subscription, so no lease or exec request is served while the active
    // map still disagrees with the control plane.
    log_recovery(&recovery::recover(provider.as_ref()).await);

    tokio::spawn(recovery::reap_loop(Arc::clone(&provider), recovery::REAP_INTERVAL));
    spawn_heartbeat_task(nats.clone(), Arc::clone(&provider), cancel.clone());

    info!(%worker_id, "sandbox-substrate-worker starting");
    Ok(run(nats, js, provider, worker_id, cancel, env).await?)
}

/// Reports how startup recovery went. A failed recovery is not fatal: the worker serves with an
/// empty active map and later passes reconsider what it could not read.
pub(super) fn log_recovery(outcome: &Result<RecoveryReport, RecoveryError>) {
    match outcome {
        Ok(report) => info!(
            adopted = report.adopted,
            pruned = report.pruned,
            retained = report.retained,
            "recovery complete"
        ),
        Err(error) => warn!(%error, "recovery failed; starting with an empty active map"),
    }
}
