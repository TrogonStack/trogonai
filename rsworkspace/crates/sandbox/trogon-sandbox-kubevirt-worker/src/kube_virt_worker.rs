use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CapabilitySupport, LeaseId, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol,
    WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, DEFAULT_HEARTBEAT_INTERVAL, NatsAuthorityOracle, RedispatchGuard,
    queue_subscribe_verified, spawn_heartbeat_loop, subjects, subscribe_verified,
};
use trogon_sandbox_ssh_gateway::TerminalHub;

use crate::constants::{DEFAULT_DRAIN_TIMEOUT, LEASE_QUEUE_GROUP};
use crate::run_warm_pool_loop::run_warm_pool_loop;
use crate::worker_authority_host::WorkerAuthorityHost;
use crate::worker_handlers::{serve_cancel, serve_checkpoint, serve_get_domain, serve_lease};
use crate::{KubeVirtWorkerError, WorkerProvider};

/// Serves one KubeVirt provider over NATS: queued lease dispatch, cancellation, port lookups, live checkpoints,
/// heartbeats, the warm pool upkeep, and the authority check that keeps the worker honest about what it holds.
pub struct KubeVirtWorker {
    client: Client,
    provider: Arc<dyn WorkerProvider>,
    terminals: Arc<TerminalHub>,
    worker_id: WorkerId,
    authority: AuthorityConfig,
    drain_timeout: Duration,
}

impl KubeVirtWorker {
    pub fn new(
        client: Client,
        provider: Arc<dyn WorkerProvider>,
        terminals: Arc<TerminalHub>,
        worker_id: WorkerId,
        authority: AuthorityConfig,
    ) -> Self {
        Self {
            client,
            provider,
            terminals,
            worker_id,
            authority,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
        }
    }

    /// How long in-flight requests may keep running after shutdown starts before they are abandoned.
    pub fn with_drain_timeout(mut self, drain_timeout: Duration) -> Self {
        self.drain_timeout = drain_timeout;
        self
    }

    /// Serves until `shutdown` completes, then lets in-flight requests finish.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), KubeVirtWorkerError> {
        let Self {
            client,
            provider,
            terminals,
            worker_id,
            authority,
            drain_timeout,
        } = self;
        let kind = ProviderKind::KubeVirt;
        let mut leases =
            queue_subscribe_verified(&client, subjects::provider_dispatch(kind), LEASE_QUEUE_GROUP).await?;
        let mut cancels = subscribe_verified(&client, subjects::provider_cancel(kind, &LeaseId::new(">"))).await?;
        let mut domains = subscribe_verified(&client, subjects::provider_get_domain(kind, &LeaseId::new("*"))).await?;
        let mut checkpoints =
            subscribe_verified(&client, subjects::provider_checkpoint_running(kind, &LeaseId::new(">"))).await?;
        client.flush().await?;
        let stop = CancellationToken::new();
        let heartbeat_provider = provider.clone();
        let heartbeat_worker = worker_id.clone();
        spawn_heartbeat_loop(
            client.clone(),
            kind,
            worker_id.clone(),
            DEFAULT_HEARTBEAT_INTERVAL,
            stop.clone(),
            move || {
                let (provider, worker_id) = (heartbeat_provider.clone(), heartbeat_worker.clone());
                async move { heartbeat(&*provider, worker_id).await }
            },
        );
        let grants = Arc::new(RedispatchGuard::new());
        let guard = AuthorityGuard::new(
            authority,
            worker_id.clone(),
            Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id)),
            Arc::new(WorkerAuthorityHost::new(
                provider.clone(),
                terminals.clone(),
                grants.clone(),
            )),
        );
        let guard_stop = stop.clone();
        let mut guard_task = tokio::spawn(async move { guard.run(guard_stop).await });
        let mut pool_task = tokio::spawn(run_warm_pool_loop(provider.warm_pool(), stop.clone()));
        let mut tasks: JoinSet<Result<(), KubeVirtWorkerError>> = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                Some(message) = leases.next() => {
                    tasks.spawn(serve_lease(client.clone(), provider.clone(), grants.clone(), message));
                }
                Some(message) = cancels.next() => {
                    tasks.spawn(serve_cancel(provider.clone(), grants.clone(), terminals.clone(), message));
                }
                Some(message) = domains.next() => {
                    tasks.spawn(serve_get_domain(client.clone(), provider.clone(), message));
                }
                Some(message) = checkpoints.next() => {
                    tasks.spawn(serve_checkpoint(client.clone(), provider.clone(), message));
                }
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Ok(Err(error)) = result {
                        tracing::warn!(%error, "KubeVirt request failed");
                    }
                }
            }
        }
        stop.cancel();
        let _ = (&mut guard_task).await;
        let _ = (&mut pool_task).await;
        leases.unsubscribe().await?;
        cancels.unsubscribe().await?;
        domains.unsubscribe().await?;
        checkpoints.unsubscribe().await?;
        let drained = tokio::time::timeout(drain_timeout, async { while tasks.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        Ok(())
    }
}

async fn heartbeat(provider: &dyn WorkerProvider, worker_id: WorkerId) -> WorkerHeartbeat {
    let profile = provider.profile();
    WorkerHeartbeat {
        worker_id,
        dispatch_protocol: WorkerDispatchProtocol::Legacy,
        native_volume_bindings: Vec::new(),
        provider: ProviderKind::KubeVirt,
        capacity: WorkerCapacity {
            max_concurrent_leases: provider.max_concurrent_leases(),
            current_leases: provider.current_leases().await,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: provider.warm_pool().population_report().await,
        supports_lease_epoch: CapabilitySupport::Unsupported,
    }
}
