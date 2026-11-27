use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, Message};
use futures::StreamExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, DEFAULT_HEARTBEAT_INTERVAL, LedgerAuthorityHost, NatsAuthorityOracle,
    nats_transfer, spawn_heartbeat_loop, subjects, subscribe_verified,
};

use crate::constants::DRAIN;
use crate::{OperationSubject, SpaceProvider, SpaceWorkerError};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Serves one SPACE provider on NATS: lease dispatches, cleanups, lease operations, the
/// heartbeat and the authority guard.
pub struct SpaceWorker {
    client: Client,
    provider: Arc<SpaceProvider>,
    authority: AuthorityConfig,
    drain: Duration,
}

impl SpaceWorker {
    /// A worker for `provider` on `client`.
    pub fn new(client: Client, provider: Arc<SpaceProvider>, authority: AuthorityConfig) -> Self {
        Self {
            client,
            provider,
            authority,
            drain: DRAIN,
        }
    }

    /// Replaces how long in-flight requests may finish after the stop signal.
    pub fn with_drain(mut self, drain: Duration) -> Self {
        self.drain = drain;
        self
    }

    /// Serves until `stop` fires, then lets in-flight requests finish and reclaims
    /// whatever sandboxes the worker still owns.
    pub async fn run(self, stop: CancellationToken) -> Result<(), SpaceWorkerError> {
        self.serve(&stop).await.map_err(SpaceWorkerError::Nats)?;
        Ok(self.provider.shutdown().await?)
    }

    async fn serve(&self, stop: &CancellationToken) -> Result<(), BoxError> {
        let ledger = Arc::clone(self.provider.dispatch_ledger());
        let worker_id = ledger.worker_id().clone();
        let mut leases =
            subscribe_verified(&self.client, subjects::worker_dispatch(ProviderKind::Space, &worker_id)).await?;
        let mut cleanups =
            subscribe_verified(&self.client, subjects::worker_cleanup(ProviderKind::Space, &worker_id)).await?;
        let mut operations = subscribe_verified(&self.client, "sandbox.provider.space.*.*").await?;
        self.client.flush().await?;

        let heartbeat_provider = Arc::clone(&self.provider);
        spawn_heartbeat_loop(
            self.client.clone(),
            ProviderKind::Space,
            worker_id.clone(),
            DEFAULT_HEARTBEAT_INTERVAL,
            stop.clone(),
            move || {
                let provider = Arc::clone(&heartbeat_provider);
                async move { provider.heartbeat().await }
            },
        );

        let cleanup_provider = Arc::clone(&self.provider);
        let host = LedgerAuthorityHost::new(ledger, move |request| {
            let provider = Arc::clone(&cleanup_provider);
            async move { Ok(provider.cleanup_dispatch(&request).await) }
        });
        let guard = AuthorityGuard::new(
            self.authority,
            worker_id.clone(),
            Arc::new(NatsAuthorityOracle::new(self.client.clone(), worker_id)),
            Arc::new(host),
        );
        let mut tasks = JoinSet::new();
        tasks.spawn(guard.run(stop.clone()));
        let mut requests = JoinSet::new();
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                Some(message) = leases.next() => {
                    requests.spawn(report(dispatch(self.client.clone(), Arc::clone(&self.provider), message)));
                }
                Some(message) = operations.next() => {
                    requests.spawn(report(operation(self.client.clone(), Arc::clone(&self.provider), message)));
                }
                Some(message) = cleanups.next() => {
                    requests.spawn(report(cleanup(self.client.clone(), Arc::clone(&self.provider), message)));
                }
                Some(_) = requests.join_next() => {}
            }
        }
        leases.unsubscribe().await?;
        cleanups.unsubscribe().await?;
        operations.unsubscribe().await?;
        let drained = tokio::time::timeout(self.drain, async { while requests.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            requests.abort_all();
            while requests.join_next().await.is_some() {}
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}

async fn report(result: impl Future<Output = Result<(), BoxError>>) {
    if let Err(error) = result.await {
        tracing::warn!(%error, "SPACE request failed");
    }
}

async fn dispatch(client: Client, provider: Arc<SpaceProvider>, message: Message) -> Result<(), BoxError> {
    let reply = provider.answer_dispatch(&message.payload).await;
    if let Some(to) = message.reply {
        client.publish(to, serde_json::to_vec(&reply)?.into()).await?;
    }
    Ok(())
}

async fn cleanup(client: Client, provider: Arc<SpaceProvider>, message: Message) -> Result<(), BoxError> {
    let response = provider.answer_cleanup(&message.payload).await?;
    if let Some(to) = message.reply {
        client.publish(to, serde_json::to_vec(&response)?.into()).await?;
    }
    Ok(())
}

async fn operation(client: Client, provider: Arc<SpaceProvider>, message: Message) -> Result<(), BoxError> {
    let subject = OperationSubject::parse(message.subject.as_str())?;
    if !provider.serves_operation(&subject).await {
        return Ok(());
    }
    let Some(to) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let response = provider.answer_operation(&subject, &payload).await?;
    nats_transfer::respond(&client, to.as_str(), serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}
