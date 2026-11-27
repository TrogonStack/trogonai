use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_nats::Client;
use futures::StreamExt;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{WorkerHeartbeat, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_dispatch, provider_token};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, FreezeOutcome, HeldLease, NatsAuthorityOracle, PurgeOutcome,
    queue_subscribe_verified, subscribe_verified,
};
use trogon_std::env::SystemEnv;

use crate::mutex_ext::MutexExt;
use crate::worker::{MemoryAuthorityHost, State, route};
use crate::{MemoryWorkerConfig, MemoryWorkerError, build_heartbeat_in_regions};

/// A provider worker that keeps every lease in memory and answers on the real provider subjects.
pub struct MemoryWorker {
    state: Arc<State>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
    authority_task: JoinHandle<()>,
    client: Client,
}

impl MemoryWorker {
    /// Subscribes to the provider subjects, then serves them in the background.
    /// The worker is ready to receive a lease dispatch when this returns.
    pub async fn start(client: Client, config: MemoryWorkerConfig) -> Result<Self, MemoryWorkerError> {
        let worker_id = WorkerId::new(uuid::Uuid::now_v7().simple().to_string())?;
        let mode = AuthorityMode::from_env(&SystemEnv)?;
        Self::start_with_authority(client, config, worker_id, AuthorityConfig::new(mode)).await
    }

    /// Like [`MemoryWorker::start`], with the worker id and the authority configuration chosen by the caller.
    pub async fn start_with_authority(
        client: Client,
        config: MemoryWorkerConfig,
        worker_id: WorkerId,
        authority: AuthorityConfig,
    ) -> Result<Self, MemoryWorkerError> {
        let kind = config.kind;
        let state = Arc::new(State::new(kind, config.commands, worker_id.clone()));
        let token = provider_token(kind);
        let lease_sub =
            queue_subscribe_verified(&client, provider_dispatch(kind), format!("sandbox-provider-{token}")).await?;
        let holder_sub = subscribe_verified(&client, format!("sandbox.provider.{token}.*.>")).await?;
        let mut subscription = futures::stream::select(lease_sub, holder_sub);
        client.flush().await?;

        let cancel = CancellationToken::new();
        let guard = AuthorityGuard::new(
            authority,
            worker_id.clone(),
            Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id)),
            Arc::new(MemoryAuthorityHost {
                state: Arc::clone(&state),
                client: client.clone(),
            }),
        );
        let authority_task = tokio::spawn(guard.run(cancel.child_token()));
        let task = tokio::spawn({
            let state = Arc::clone(&state);
            let cancel = cancel.clone();
            let client = client.clone();
            async move {
                let mut requests = JoinSet::new();
                tracing::info!(%token, "memory worker ready");
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        message = subscription.next() => {
                            let Some(message) = message else { break };
                            let (state, client) = (Arc::clone(&state), client.clone());
                            requests.spawn(async move {
                                if let Err(error) = route(state, &client, message).await {
                                    tracing::warn!(%error, "memory worker failed to handle a message");
                                }
                            });
                        }
                        _ = requests.join_next(), if !requests.is_empty() => {}
                    }
                }
                requests.abort_all();
                while requests.join_next().await.is_some() {}
            }
        });
        Ok(Self {
            state,
            cancel,
            task,
            authority_task,
            client,
        })
    }

    /// The id this worker heartbeats and answers under.
    pub fn worker_id(&self) -> &WorkerId {
        &self.state.worker_id
    }

    /// How many leases are currently held.
    pub fn lease_count(&self) -> usize {
        self.state.leases.lock_clean().len()
    }

    /// The leases currently held, as the authority guard sees them.
    pub fn authority_held_leases(&self) -> Vec<HeldLease> {
        self.state.held_leases()
    }

    /// Closes `lease` the way the authority guard does when another worker owns it.
    pub async fn authority_purge(&self, lease: &HeldLease) -> PurgeOutcome {
        self.state.purge(&self.client, lease).await
    }

    /// Asks to freeze `lease` the way the authority guard does.
    pub async fn authority_freeze(&self, lease: &HeldLease) -> FreezeOutcome {
        self.state.freeze(lease).await
    }

    /// Builds this worker's heartbeat each time it is called, claiming placement in `regions`.
    pub fn heartbeat_source(
        &self,
        max_concurrent_leases: u32,
        regions: Vec<String>,
    ) -> impl Fn() -> WorkerHeartbeat + Send + Sync + 'static {
        let state = Arc::clone(&self.state);
        move || {
            build_heartbeat_in_regions(
                state.kind,
                &state.worker_id,
                max_concurrent_leases,
                u32::try_from(state.leases.lock_clean().len()).unwrap_or(u32::MAX),
                &regions,
            )
        }
    }

    /// Stops serving and closes every lease.
    pub async fn shutdown(self) {
        self.state.shutting_down.store(true, Ordering::SeqCst);
        self.cancel.cancel();
        let _ = self.task.await;
        let _ = self.authority_task.await;
        let leases: Vec<_> = self.state.leases.lock_clean().keys().cloned().collect();
        for lease in leases {
            self.state.close_lease(&self.client, &lease).await;
        }
    }
}
