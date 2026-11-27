use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CapabilitySupport, ProviderCleanupRequest, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind,
    WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityHostError, DEFAULT_HEARTBEAT_INTERVAL, LedgerAuthorityHost,
    NatsAuthorityOracle, decode_verified, spawn_heartbeat_loop, subjects, subscribe_verified,
};

use crate::constants::{DEFAULT_DRAIN_TIMEOUT, OPERATIONS_SUBJECT};
use crate::worker_operations::handle;
use crate::{FlyProvider, FlyWorkerError};

/// Serves one Fly provider over NATS: lease dispatch, cleanup, lease operations, heartbeats and the authority
/// check that keeps the worker honest about what it holds.
pub struct FlyWorker {
    client: Client,
    provider: Arc<FlyProvider>,
    authority: AuthorityConfig,
    drain_timeout: Duration,
}

impl FlyWorker {
    pub fn new(client: Client, provider: Arc<FlyProvider>, authority: AuthorityConfig) -> Self {
        Self {
            client,
            provider,
            authority,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
        }
    }

    /// How long in-flight requests may keep running after shutdown starts before they are abandoned.
    pub fn with_drain_timeout(mut self, drain_timeout: Duration) -> Self {
        self.drain_timeout = drain_timeout;
        self
    }

    /// Serves until `shutdown` completes, then lets in-flight requests finish and destroys everything the
    /// provider owns.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), FlyWorkerError> {
        let Self {
            client,
            provider,
            authority,
            drain_timeout,
        } = self;
        let ledger = provider.dispatch_ledger().clone();
        let worker_id = ledger.worker_id().clone();
        let mut leases = subscribe_verified(&client, subjects::worker_dispatch(ProviderKind::Fly, &worker_id)).await?;
        let mut cleanups = subscribe_verified(&client, subjects::worker_cleanup(ProviderKind::Fly, &worker_id)).await?;
        let mut operations = subscribe_verified(&client, OPERATIONS_SUBJECT).await?;
        client.flush().await?;
        let stop = CancellationToken::new();
        let heartbeat_provider = provider.clone();
        spawn_heartbeat_loop(
            client.clone(),
            ProviderKind::Fly,
            worker_id.clone(),
            DEFAULT_HEARTBEAT_INTERVAL,
            stop.clone(),
            move || {
                let provider = heartbeat_provider.clone();
                async move { heartbeat(&provider).await }
            },
        );
        let cleanup_provider = provider.clone();
        let host = LedgerAuthorityHost::new(ledger, move |request: ProviderCleanupRequest| {
            let provider = cleanup_provider.clone();
            async move { Ok::<_, AuthorityHostError>(provider.cleanup_dispatch(&request).await) }
        });
        let guard = AuthorityGuard::new(
            authority,
            worker_id.clone(),
            Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id.clone())),
            Arc::new(host),
        );
        let guard_stop = stop.clone();
        let mut guard_task = tokio::spawn(async move { guard.run(guard_stop).await });
        let mut pool_tick = tokio::time::interval(DEFAULT_HEARTBEAT_INTERVAL);
        let mut tasks: JoinSet<Result<(), FlyWorkerError>> = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                _ = pool_tick.tick() => {
                    let provider = provider.clone();
                    tasks.spawn(async move { Ok(provider.maintain_pool().await?) });
                }
                Some(message) = leases.next() => {
                    tasks.spawn(serve_lease(client.clone(), provider.clone(), worker_id.clone(), message));
                }
                Some(message) = operations.next() => {
                    let (client, provider) = (client.clone(), provider.clone());
                    tasks.spawn(async move { handle(&client, &provider, message).await });
                }
                Some(message) = cleanups.next() => {
                    tasks.spawn(serve_cleanup(client.clone(), provider.clone(), message));
                }
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Ok(Err(error)) = result {
                        tracing::warn!(%error, "Fly request failed");
                    }
                }
            }
        }
        stop.cancel();
        let _ = (&mut guard_task).await;
        leases.unsubscribe().await?;
        cleanups.unsubscribe().await?;
        operations.unsubscribe().await?;
        let drained = tokio::time::timeout(drain_timeout, async { while tasks.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        Ok(provider.shutdown().await?)
    }
}

async fn heartbeat(provider: &FlyProvider) -> WorkerHeartbeat {
    let profile = provider.profile();
    WorkerHeartbeat {
        worker_id: provider.dispatch_ledger().worker_id().clone(),
        dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: provider.native_volume_bindings(),
        provider: ProviderKind::Fly,
        capacity: WorkerCapacity {
            max_concurrent_leases: u32::try_from(provider.config().max_leases()).unwrap_or(u32::MAX),
            current_leases: u32::try_from(provider.current_leases().await).unwrap_or(u32::MAX),
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: std::collections::HashMap::new(),
        supports_lease_epoch: CapabilitySupport::Supported,
    }
}

async fn serve_lease(
    client: Client,
    provider: Arc<FlyProvider>,
    worker_id: trogon_sandbox_contract::WorkerId,
    message: async_nats::Message,
) -> Result<(), FlyWorkerError> {
    let reply = match decode_verified::<ProviderDispatchPayload>(&message.payload, Some(&worker_id)) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(dispatch) if dispatch.target.is_none() => ProviderDispatchReply::LaunchFailed {
            provider_error: "targeted dispatch identity is required".to_owned(),
        },
        Ok(dispatch) => match provider.lease_dispatch(&dispatch).await {
            Ok(lease) => ProviderDispatchReply::Granted(lease),
            Err(error) => ProviderDispatchReply::LaunchFailed {
                provider_error: error.to_string(),
            },
        },
    };
    if let Some(subject) = message.reply {
        client.publish(subject, serde_json::to_vec(&reply)?.into()).await?;
    }
    Ok(())
}

async fn serve_cleanup(
    client: Client,
    provider: Arc<FlyProvider>,
    message: async_nats::Message,
) -> Result<(), FlyWorkerError> {
    let request: ProviderCleanupRequest = serde_json::from_slice(&message.payload)?;
    let response = provider.cleanup_dispatch(&request).await;
    if let Some(subject) = message.reply {
        client.publish(subject, serde_json::to_vec(&response)?.into()).await?;
    }
    Ok(())
}
