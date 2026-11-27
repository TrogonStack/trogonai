use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_nats::Client;
use futures::StreamExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat};
use trogon_sandbox_protocol::subjects;

use crate::constants::{HEARTBEAT_FRESHNESS, PROVIDER_KINDS};
use crate::{CapabilitiesResponse, ProviderCapabilityEntry};

/// Tracks which providers have a live worker, fed by worker heartbeats.
#[derive(Default)]
pub struct CapabilityRegistry {
    live: RwLock<HashMap<ProviderKind, (u32, u32, SystemTime)>>,
}

impl CapabilityRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn snapshot(&self) -> CapabilitiesResponse {
        let live = self.live.read().await;
        let now = SystemTime::now();
        let providers = PROVIDER_KINDS
            .iter()
            .map(|&kind| {
                let (workers, caps) = live
                    .get(&kind)
                    .filter(|(_, _, seen)| now.duration_since(*seen).is_ok_and(|age| age <= HEARTBEAT_FRESHNESS))
                    .map_or((0, 0), |(workers, caps, _)| (*workers, *caps));
                ProviderCapabilityEntry {
                    kind,
                    isolation_tiers: kind.isolation_tiers().to_vec(),
                    live_workers: workers,
                    worker_capabilities: caps,
                }
            })
            .collect();
        CapabilitiesResponse { providers }
    }

    pub async fn record_heartbeat(&self, heartbeat: WorkerHeartbeat) {
        let caps = heartbeat.capacity.capabilities.bits();
        self.live
            .write()
            .await
            .insert(heartbeat.provider, (1, caps, heartbeat.emitted_at));
    }

    /// Follows every provider's heartbeat subject until `cancel` fires.
    pub fn spawn_heartbeat_watcher(client: Client, registry: Arc<Self>, cancel: CancellationToken) {
        for &provider in PROVIDER_KINDS {
            tokio::spawn(watch_provider(
                client.clone(),
                Arc::clone(&registry),
                cancel.clone(),
                provider,
            ));
        }
    }
}

async fn watch_provider(
    client: Client,
    registry: Arc<CapabilityRegistry>,
    cancel: CancellationToken,
    provider: ProviderKind,
) {
    let Ok(mut subscription) = client.subscribe(subjects::worker_heartbeat_filter(provider)).await else {
        return;
    };
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            message = subscription.next() => {
                let Some(message) = message else { break };
                if let Ok(heartbeat) = serde_json::from_slice::<WorkerHeartbeat>(&message.payload) {
                    registry.record_heartbeat(heartbeat).await;
                }
            }
        }
    }
}
