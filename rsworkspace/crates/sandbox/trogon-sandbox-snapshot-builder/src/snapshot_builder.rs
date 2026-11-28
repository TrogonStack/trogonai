use std::sync::Arc;
use std::time::SystemTime;

use async_nats::jetstream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_broker::snapshot_accounting::SnapshotAccountingStore;
use trogon_sandbox_broker::snapshot_capture_status::{NatsKvSnapshotCaptureStatusStore, SnapshotCaptureStatusStore};
use trogon_sandbox_broker::snapshot_tag::{JetStreamSnapshotTagAuditor, NatsKvSnapshotTagStore, SnapshotTagStore};
use trogon_sandbox_contract::LeaseReleased;
use trogon_sandbox_oci::artifact_type_label;
use trogon_sandbox_protocol::subjects::LEASE_RELEASED_FILTER;

use crate::constants::{RELEASE_CHANNEL_CAPACITY, RELEASE_QUEUE_GROUP, TAG_AUDIT_JETSTREAM_REPLICAS};
use crate::release_forwarder::forward_releases;
use crate::release_outcome::{ReleaseOutcome, capture_status};
use crate::release_processor::ReleaseProcessor;
use crate::{BuilderConfig, ExportError, SnapshotBuilderError, SnapshotBuilderMetrics, SnapshotKvStore, TagRecovery};

/// Subscribes to lease releases and publishes, secures, and tags the snapshot each one asks for.
pub struct SnapshotBuilder {
    processor: Arc<ReleaseProcessor>,
    tag_store: Arc<dyn SnapshotTagStore>,
    capture_status_store: Arc<dyn SnapshotCaptureStatusStore>,
    tag_recovery: Arc<TagRecovery>,
    metrics: SnapshotBuilderMetrics,
}

impl SnapshotBuilder {
    pub async fn connect(config: BuilderConfig, metrics: SnapshotBuilderMetrics) -> Result<Self, SnapshotBuilderError> {
        let nats = async_nats::connect(&config.nats_url)
            .await
            .map_err(|source| SnapshotBuilderError::Connect {
                url: config.nats_url.clone(),
                source,
            })?;
        let js = jetstream::new(nats.clone());
        let kv = SnapshotKvStore::open(js.clone()).await?;
        let audit = Arc::new(JetStreamSnapshotTagAuditor::new(js.clone(), TAG_AUDIT_JETSTREAM_REPLICAS).await?);
        let tag_store: Arc<dyn SnapshotTagStore> = Arc::new(NatsKvSnapshotTagStore::new(js.clone(), audit).await?);
        let accounting = SnapshotAccountingStore::open(js.clone()).await?;
        let tag_recovery = Arc::new(TagRecovery::open(js.clone(), nats.clone()).await?);
        let capture_status_store: Arc<dyn SnapshotCaptureStatusStore> =
            Arc::new(NatsKvSnapshotCaptureStatusStore::new(js).await?);
        Ok(Self {
            processor: Arc::new(ReleaseProcessor {
                nats,
                kv,
                tag_store: Arc::clone(&tag_store),
                accounting,
                config,
            }),
            tag_store,
            capture_status_store,
            tag_recovery,
            metrics,
        })
    }

    /// The NATS connection the builder serves on.
    pub fn nats(&self) -> &async_nats::Client {
        &self.processor.nats
    }

    pub fn metrics(&self) -> SnapshotBuilderMetrics {
        self.metrics.clone()
    }

    pub async fn run(self, cancel: CancellationToken) -> Result<(), SnapshotBuilderError> {
        let config = &self.processor.config;
        let subscription = self
            .processor
            .nats
            .queue_subscribe(LEASE_RELEASED_FILTER, RELEASE_QUEUE_GROUP.to_string())
            .await?;
        info!(
            subject = LEASE_RELEASED_FILTER,
            "snapshot builder subscribed to lease releases"
        );

        let (queue, mut releases) = mpsc::channel::<(LeaseReleased, SystemTime)>(RELEASE_CHANNEL_CAPACITY);
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent.get()));
        let recovery_cancel = cancel.child_token();
        let mut recovery =
            tokio::spawn(Arc::clone(&self.tag_recovery).run(Arc::clone(&self.tag_store), recovery_cancel.clone()));
        let mut recovery_result = None;
        let subscriber = tokio::spawn(forward_releases(
            subscription,
            queue,
            cancel.clone(),
            self.metrics.clone(),
        ));

        info!(
            max_concurrent = config.max_concurrent.get(),
            default_registry = %config.default_registry,
            "snapshot builder ready"
        );

        loop {
            tokio::select! {
                result = &mut recovery => {
                    recovery_result = Some(result);
                    break;
                }
                _ = cancel.cancelled() => {
                    info!("snapshot builder shutting down");
                    break;
                }
                next = releases.recv() => {
                    let Some((event, observed_at)) = next else { break };
                    self.metrics.queue_depth.set(i64::try_from(releases.len()).unwrap_or(i64::MAX));
                    let permit = tokio::select! {
                        _ = cancel.cancelled() => break,
                        permit = Arc::clone(&semaphore).acquire_owned() => match permit {
                            Ok(permit) => permit,
                            Err(_) => break,
                        },
                    };
                    tokio::spawn(handle_release(
                        Arc::clone(&self.processor),
                        Arc::clone(&self.capture_status_store),
                        self.metrics.clone(),
                        event,
                        observed_at,
                        permit,
                    ));
                }
            }
        }

        subscriber.abort();
        let _ = subscriber.await;
        recovery_cancel.cancel();
        match recovery_result {
            Some(result) => result??,
            None => recovery.await??,
        }
        Ok(())
    }
}

async fn handle_release(
    processor: Arc<ReleaseProcessor>,
    capture_status_store: Arc<dyn SnapshotCaptureStatusStore>,
    metrics: SnapshotBuilderMetrics,
    event: LeaseReleased,
    observed_at: SystemTime,
    permit: OwnedSemaphorePermit,
) {
    metrics.inflight.inc();
    let result = match event.snapshot_out.as_ref() {
        Some(policy) => processor.process(&event, policy, observed_at).await,
        None => Err(ExportError::Permanent(
            "snapshot_out missing on release event".to_string(),
        )),
    };
    metrics.inflight.dec();
    drop(permit);

    let status = capture_status(&result);
    if let Err(error) = capture_status_store.record(&event.lease_id, status).await {
        warn!(lease_id = %event.lease_id.as_str(), %error, "failed to record snapshot capture status");
    }
    report(&metrics, &event, result);
}

fn report(metrics: &SnapshotBuilderMetrics, event: &LeaseReleased, result: Result<ReleaseOutcome, ExportError>) {
    match result {
        Ok(outcome) => {
            if let Some(error) = outcome.tag_failure {
                warn!(lease_id = %event.lease_id.as_str(), %error, "snapshot tag not moved");
                metrics.failed_total.with_label_values(&[error.reason_label()]).inc();
            }
            metrics
                .pushed_total
                .with_label_values(&[artifact_type_label(outcome.receipt.artifact_type)])
                .inc();
        }
        Err(error) => {
            warn!(lease_id = %event.lease_id.as_str(), %error, "snapshot export failed");
            metrics.failed_total.with_label_values(&[error.reason_label()]).inc();
        }
    }
}
