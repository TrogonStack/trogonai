use std::collections::HashMap;
use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, Provider, RedispatchGuard,
    queue_subscribe_verified, subscribe_verified,
};
use trogon_std::env::SystemEnv;

use crate::cancel_dispatch::handle_cancel_msg;
use crate::constants as subjects;
use crate::lease_dispatch::handle_lease_msg;
use crate::lease_verbs::serve_lease_verbs;
use crate::snapshot_export::serve_snapshot_export;
use crate::{DaytonaWorkerError, DaytonaWorkerProvider};

/// Serves `daytona` over NATS as `worker_id` until `cancel` fires: lease dispatch, cancel, the per-lease verbs and the
/// authority check that keeps the worker honest about what it holds.
pub async fn run(
    client: Client,
    js: jetstream::Context,
    daytona: Arc<DaytonaWorkerProvider>,
    worker_id: &WorkerId,
    cancel: CancellationToken,
) -> Result<(), DaytonaWorkerError> {
    daytona.bind_worker(worker_id)?;
    let authority_config = AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?);
    let provider: Arc<dyn Provider> = daytona.clone();
    serve_lease_verbs(&client, &daytona, &cancel).await?;
    serve_snapshot_export(&client, worker_id, &cancel).await?;
    let mut leases = queue_subscribe_verified(&client, subjects::LEASE, subjects::LEASE_QUEUE_GROUP).await?;
    let mut cancels = subscribe_verified(&client, subjects::CANCEL).await?;
    let active = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let authority = AuthorityGuard::new(
        authority_config,
        worker_id.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id.clone())),
        daytona,
    );
    tokio::spawn(authority.run(cancel.clone()));
    info!("worker ready");
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                info!("worker shutting down");
                break;
            }
            next = leases.next() => {
                let Some(message) = next else { break };
                let (client, js, provider) = (client.clone(), js.clone(), Arc::clone(&provider));
                let (active, grants) = (Arc::clone(&active), Arc::clone(&grants));
                tokio::spawn(async move {
                    let _ = handle_lease_msg(message, client, js, provider, active, grants).await.inspect_err(|e| warn!(error = %e, "failed to handle lease request"));
                });
            }
            next = cancels.next() => {
                let Some(message) = next else { break };
                let (provider, active, grants) = (Arc::clone(&provider), Arc::clone(&active), Arc::clone(&grants));
                tokio::spawn(handle_cancel_msg(message, provider, active, grants));
            }
        }
    }
    Ok(())
}
