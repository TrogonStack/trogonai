use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use futures::stream::{BoxStream, select_all};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{LeaseId, ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, Provider, RedispatchGuard,
    queue_subscribe_verified, subscribe_verified,
};
use trogon_std::env::SystemEnv;

use crate::cancel_dispatch::handle_cancel;
use crate::constants::{CLEANUP_RETRY_INTERVAL, LEASE_QUEUE_GROUP};
use crate::lease_dispatch::handle_lease;
use crate::worker_event::WorkerEvent;
use crate::{AcaWorkerProvider, WorkerError, run_cleanup_retries};

/// Serves lease dispatches and cancels for `worker` until `cancel` fires.
pub async fn run_worker(
    client: Client,
    provider: Arc<AcaWorkerProvider>,
    worker: WorkerId,
    cancel: CancellationToken,
) -> Result<(), WorkerError> {
    provider.bind_worker(&worker)?;
    let authority = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?),
        worker.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), worker)),
        provider.clone(),
    );
    tokio::spawn(authority.run(cancel.clone()));
    tokio::spawn(run_cleanup_retries(
        provider.clone(),
        CLEANUP_RETRY_INTERVAL,
        cancel.clone(),
    ));

    let lease_subject = provider_dispatch(ProviderKind::AzureContainerApps);
    let cancel_subject = provider_cancel(ProviderKind::AzureContainerApps, &LeaseId::new(">"));
    let streams: Vec<BoxStream<'static, WorkerEvent>> = vec![
        queue_subscribe_verified(&client, lease_subject.clone(), LEASE_QUEUE_GROUP)
            .await?
            .map(WorkerEvent::Lease)
            .boxed(),
        subscribe_verified(&client, cancel_subject.clone())
            .await?
            .map(WorkerEvent::Cancel)
            .boxed(),
    ];
    let mut events = select_all(streams);
    let grants = Arc::new(RedispatchGuard::new());
    let provider: Arc<dyn Provider> = provider;
    info!(%lease_subject, %cancel_subject, "worker ready");

    while let Some(event) = tokio::select! {
        _ = cancel.cancelled() => None,
        event = events.next() => event,
    } {
        match event {
            WorkerEvent::Lease(msg) => {
                tokio::spawn(handle_lease(msg, client.clone(), provider.clone(), grants.clone()));
            }
            WorkerEvent::Cancel(msg) => {
                tokio::spawn(handle_cancel(msg, provider.clone(), grants.clone()));
            }
        }
    }
    info!("worker shutting down");
    Ok(())
}
