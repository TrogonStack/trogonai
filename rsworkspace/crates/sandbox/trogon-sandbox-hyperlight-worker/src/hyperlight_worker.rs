use std::collections::HashMap;
use std::sync::Arc;

use async_nats::jetstream;
use async_nats::{Client, Message, Subject};
use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseId, LifecycleEvent, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId, SandboxPhase, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, DEFAULT_HEARTBEAT_INTERVAL, NatsAuthorityOracle, PhaseTimer, Provider,
    ProviderError, RedispatchGuard, decode_verified, lifecycle_subject, queue_subscribe_verified, spawn_heartbeat_loop,
    subjects, subscribe_verified,
};

use crate::cancel_subject::extract_cancel_lease_id;
use crate::constants::QUEUE_GROUP;
use crate::lease_error_reply::LeaseErrorReply;
use crate::{HyperlightWorkerError, HyperlightWorkerProvider, Hypervisor};

/// Serves one Hyperlight provider on NATS: lease dispatches, cancels, the heartbeat and the
/// authority guard.
pub struct HyperlightWorker<H: Hypervisor> {
    client: Client,
    provider: Arc<HyperlightWorkerProvider<H>>,
    authority: AuthorityConfig,
    worker_id: WorkerId,
}

/// What each request handler needs, shared across the tasks of one run.
struct Serving<H: Hypervisor> {
    client: Client,
    js: jetstream::Context,
    provider: Arc<HyperlightWorkerProvider<H>>,
    active: Mutex<HashMap<LeaseId, CancellationToken>>,
    grants: RedispatchGuard,
}

impl<H: Hypervisor> HyperlightWorker<H> {
    /// A worker named `worker_id` that serves `provider` on `client`.
    pub fn new(
        client: Client,
        provider: Arc<HyperlightWorkerProvider<H>>,
        authority: AuthorityConfig,
        worker_id: WorkerId,
    ) -> Self {
        Self {
            client,
            provider,
            authority,
            worker_id,
        }
    }

    /// Serves until `stop` fires.
    pub async fn run(self, stop: CancellationToken) -> Result<(), HyperlightWorkerError> {
        self.provider.bind_worker(&self.worker_id)?;
        let guard = AuthorityGuard::new(
            self.authority,
            self.worker_id.clone(),
            Arc::new(NatsAuthorityOracle::new(self.client.clone(), self.worker_id.clone())),
            self.provider.clone(),
        );
        let authority = tokio::spawn(guard.run(stop.clone()));
        let lease_subject = subjects::provider_dispatch(ProviderKind::Hyperlight);
        let cancel_subject = subjects::provider_cancel(ProviderKind::Hyperlight, &LeaseId::new(">"));
        let mut leases = queue_subscribe_verified(&self.client, lease_subject.clone(), QUEUE_GROUP).await?;
        let mut cancels = subscribe_verified(&self.client, cancel_subject.clone()).await?;
        self.client.flush().await?;
        self.spawn_heartbeat(stop.clone());
        tracing::info!(%lease_subject, %cancel_subject, "worker ready");

        let serving = Arc::new(Serving {
            client: self.client.clone(),
            js: jetstream::new(self.client.clone()),
            provider: Arc::clone(&self.provider),
            active: Mutex::new(HashMap::new()),
            grants: RedispatchGuard::new(),
        });
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                Some(message) = leases.next() => {
                    tasks.spawn(Arc::clone(&serving).handle_lease(message));
                }
                Some(message) = cancels.next() => {
                    tasks.spawn(Arc::clone(&serving).handle_cancel(message));
                }
                Some(_) = tasks.join_next() => {}
            }
        }
        tracing::info!("worker shutting down");
        leases.unsubscribe().await?;
        cancels.unsubscribe().await?;
        drop(tasks);
        let _ = authority.await;
        Ok(())
    }

    fn spawn_heartbeat(&self, stop: CancellationToken) {
        let provider = Arc::clone(&self.provider);
        let worker_id = self.worker_id.clone();
        spawn_heartbeat_loop(
            self.client.clone(),
            ProviderKind::Hyperlight,
            self.worker_id.clone(),
            DEFAULT_HEARTBEAT_INTERVAL,
            stop,
            move || {
                let heartbeat = provider.heartbeat(&worker_id);
                async move { heartbeat }
            },
        );
    }
}

impl<H: Hypervisor> Serving<H> {
    async fn publish_lifecycle(&self, subject: &str, event: &LifecycleEvent) -> Result<(), HyperlightWorkerError> {
        self.js
            .publish(subject.to_owned(), serde_json::to_vec(event)?.into())
            .await?;
        Ok(())
    }

    async fn emit(&self, subject: &str, event: LifecycleEvent) {
        self.publish_lifecycle(subject, &event)
            .await
            .inspect_err(|error| tracing::warn!(%error, subject, "failed to publish lifecycle event"))
            .ok();
    }

    async fn reply(
        &self,
        to: Option<Subject>,
        body: Result<Vec<u8>, serde_json::Error>,
    ) -> Result<(), HyperlightWorkerError> {
        if let Some(to) = to {
            self.client.publish(to, Bytes::from(body?)).await?;
        }
        Ok(())
    }

    async fn launch(&self, payload: &ProviderDispatchPayload) -> Result<ProviderLease, ProviderError> {
        let lease_id = &payload.lease_id;
        let sandbox_id = SandboxId::new(lease_id.as_str()).map_err(ProviderError::backend)?;
        let subject = lifecycle_subject(lease_id);
        self.emit(
            &subject,
            LifecycleEvent::Started {
                sandbox_id: sandbox_id.clone(),
            },
        )
        .await;

        let token = CancellationToken::new();
        self.active.lock().await.insert(lease_id.clone(), token);
        let allocate = PhaseTimer::start(SandboxPhase::Allocate);
        self.emit(&subject, allocate.phase_started()).await;

        let result = self.provider.lease(&payload.request, lease_id).await;
        match &result {
            Ok(lease) => {
                self.emit(&subject, allocate.complete()).await;
                tracing::info!(lease_id = lease_id.as_str(), sandbox_id = %lease.sandbox_id, "hyperlight sandbox launched");
                self.emit(
                    &subject,
                    LifecycleEvent::Completed {
                        sandbox_id: lease.sandbox_id.clone(),
                    },
                )
                .await;
            }
            Err(error) => {
                self.emit(&subject, allocate.fail(error.to_string())).await;
                tracing::warn!(lease_id = lease_id.as_str(), %error, "hyperlight sandbox launch failed");
                self.emit(
                    &subject,
                    LifecycleEvent::Failed {
                        sandbox_id,
                        error: error.to_string(),
                    },
                )
                .await;
            }
        }
        self.active.lock().await.remove(lease_id);
        result
    }

    async fn handle_lease(self: Arc<Self>, message: Message) {
        let reply = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
            Ok(payload) => {
                tracing::info!(lease_id = payload.lease_id.as_str(), tenant = %payload.request.tenant, "lease accepted");
                // A redelivered dispatch gets the first grant instead of a second sandbox.
                let granted = self
                    .grants
                    .get_or_launch(&payload.lease_id, || self.launch(&payload))
                    .await;
                match granted {
                    Ok(lease) => serde_json::to_vec(&lease),
                    Err(error) => serde_json::to_vec(&LeaseErrorReply::ProviderError(error.to_string())),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "invalid dispatch payload");
                serde_json::to_vec(&LeaseErrorReply::DecodeError(error.to_string()))
            }
        };
        self.reply(message.reply, reply)
            .await
            .inspect_err(|error| tracing::warn!(%error, "failed to reply to a lease request"))
            .ok();
    }

    async fn handle_cancel(self: Arc<Self>, message: Message) {
        match extract_cancel_lease_id(message.subject.as_str()) {
            Some(lease_id) => self.cancel(&lease_id).await,
            None => tracing::warn!(subject = message.subject.as_str(), "cancel names no lease"),
        }
    }

    async fn cancel(&self, lease_id: &LeaseId) {
        self.grants.forget(lease_id);
        if let Some(token) = self.active.lock().await.get(lease_id) {
            token.cancel();
        }
        match self.provider.cancel(lease_id).await {
            Ok(()) => tracing::info!(lease_id = lease_id.as_str(), "lease released"),
            Err(error) => tracing::warn!(lease_id = lease_id.as_str(), %error, "cancel failed"),
        }
    }
}
