use std::sync::{Arc, Mutex};

use async_nats::Client;
use async_trait::async_trait;

use super::build::AnomalyIngressSnapshot;
use super::errors::AnomalyError;
use super::features::AnomalyFeatures;
use super::novelty::NoveltyTracker;
use super::rate::RateTracker;

pub fn subject_for_tenant(prefix: &str, tenant_id: &str) -> String {
    format!("{prefix}.anomaly.features.{tenant_id}")
}

/// Publishes anomaly feature vectors to NATS (best-effort from ingress).
#[async_trait]
pub trait AnomalyEmit: Send + Sync {
    async fn emit(&self, features: &AnomalyFeatures) -> Result<(), AnomalyError>;

    async fn emit_ingress(&self, snapshot: &AnomalyIngressSnapshot) -> Result<(), AnomalyError>;
}

#[derive(Clone)]
pub struct AnomalyEmitter {
    prefix: Arc<str>,
    client: Client,
    novelty: Arc<Mutex<NoveltyTracker>>,
    rate: Arc<Mutex<RateTracker>>,
}

impl AnomalyEmitter {
    #[must_use]
    pub fn new(prefix: impl Into<Arc<str>>, client: Client) -> Self {
        Self {
            prefix: prefix.into(),
            client,
            novelty: Arc::new(Mutex::new(NoveltyTracker::default())),
            rate: Arc::new(Mutex::new(RateTracker::default())),
        }
    }

    fn build_from_snapshot(
        &self,
        snapshot: &AnomalyIngressSnapshot,
    ) -> Result<AnomalyFeatures, AnomalyError> {
        let mut novelty = self
            .novelty
            .lock()
            .map_err(|_| AnomalyError::TransportFailed("anomaly novelty tracker lock poisoned".into()))?;
        let mut rate = self
            .rate
            .lock()
            .map_err(|_| AnomalyError::TransportFailed("anomaly rate tracker lock poisoned".into()))?;
        snapshot.build_features(&mut novelty, &mut rate)
    }
}

#[async_trait]
impl AnomalyEmit for AnomalyEmitter {
    async fn emit_ingress(&self, snapshot: &AnomalyIngressSnapshot) -> Result<(), AnomalyError> {
        let features = self.build_from_snapshot(snapshot)?;
        self.emit(&features).await
    }

    async fn emit(&self, features: &AnomalyFeatures) -> Result<(), AnomalyError> {
        let subject = subject_for_tenant(self.prefix.as_ref(), &features.tenant_id);
        let payload =
            serde_json::to_vec(features).map_err(AnomalyError::SerializeFailed)?;
        self.client
            .publish(subject, payload.into())
            .await
            .map_err(|err| AnomalyError::TransportFailed(err.to_string()))?;
        self.client
            .flush()
            .await
            .map_err(|err| AnomalyError::TransportFailed(err.to_string()))?;
        Ok(())
    }
}
