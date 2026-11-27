//! The periodic worker heartbeat loop.

use std::future::Future;
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use trogon_nats::PublishClient;
use trogon_sandbox_contract::{CapabilitySupport, ProviderKind, WorkerHeartbeat, WorkerId};

pub use crate::constants::DEFAULT_HEARTBEAT_INTERVAL;
use crate::subjects;

/// Publishes a [`WorkerHeartbeat`] on [`subjects::worker_heartbeat`] every `interval`, starting
/// with an immediate tick, until `cancel` fires.
///
/// `build` runs on every tick so each worker keeps owning what its heartbeat says (capacity,
/// warm pool, profile). A serialize or publish failure is logged and the loop keeps going, so
/// one bad tick never silences the worker.
///
/// Every worker on this loop receives through the verified subscription path, which tracks
/// lease epochs, so the loop stamps `supports_lease_epoch` itself and `build` need not.
pub fn spawn_heartbeat_loop<C, F, Fut>(
    client: C,
    provider_kind: ProviderKind,
    worker_id: WorkerId,
    interval: Duration,
    cancel: CancellationToken,
    build: F,
) where
    C: PublishClient,
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = WorkerHeartbeat> + Send,
{
    tokio::spawn(async move {
        let subject = subjects::worker_heartbeat(provider_kind, &worker_id);
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let mut heartbeat = build().await;
                    heartbeat.supports_lease_epoch = CapabilitySupport::Supported;
                    match serde_json::to_vec(&heartbeat) {
                        Ok(payload) => {
                            if let Err(error) = client
                                .publish_with_headers(subject.clone(), HeaderMap::new(), Bytes::from(payload))
                                .await
                            {
                                tracing::warn!(%error, "failed to publish heartbeat");
                            }
                        }
                        Err(error) => tracing::warn!(%error, "failed to serialize heartbeat"),
                    }
                }
                _ = cancel.cancelled() => break,
            }
        }
    });
}
