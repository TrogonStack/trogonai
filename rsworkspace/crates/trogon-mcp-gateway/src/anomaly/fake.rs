//! Test double for ingress anomaly emission (captures features or forces errors).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::build::AnomalyIngressSnapshot;
use super::errors::AnomalyError;
use super::features::AnomalyFeatures;
use super::novelty::NoveltyTracker;
use super::rate::RateTracker;
use super::AnomalyEmit;

/// Records emitted feature vectors; optionally fails every `emit` call.
#[derive(Clone)]
pub struct FakeAnomalyEmitter {
    captured: Arc<Mutex<Vec<AnomalyFeatures>>>,
    fail: Arc<AtomicBool>,
    novelty: Arc<Mutex<NoveltyTracker>>,
    rate: Arc<Mutex<RateTracker>>,
}

impl FakeAnomalyEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            captured: Arc::new(Mutex::new(Vec::new())),
            fail: Arc::new(AtomicBool::new(false)),
            novelty: Arc::new(Mutex::new(NoveltyTracker::default())),
            rate: Arc::new(Mutex::new(RateTracker::default())),
        }
    }

    pub fn set_fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    pub fn captured_features(&self) -> Vec<AnomalyFeatures> {
        self.captured
            .lock()
            .expect("fake anomaly emitter mutex poisoned")
            .clone()
    }

    fn build_from_snapshot(
        &self,
        snapshot: &AnomalyIngressSnapshot,
    ) -> Result<AnomalyFeatures, AnomalyError> {
        let mut novelty = self
            .novelty
            .lock()
            .expect("fake anomaly emitter novelty mutex poisoned");
        let mut rate = self
            .rate
            .lock()
            .expect("fake anomaly emitter rate mutex poisoned");
        snapshot.build_features(&mut novelty, &mut rate)
    }
}

impl Default for FakeAnomalyEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AnomalyEmit for FakeAnomalyEmitter {
    async fn emit_ingress(&self, snapshot: &AnomalyIngressSnapshot) -> Result<(), AnomalyError> {
        let features = self.build_from_snapshot(snapshot)?;
        self.emit(&features).await
    }

    async fn emit(&self, features: &AnomalyFeatures) -> Result<(), AnomalyError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(AnomalyError::TransportFailed("injected test failure".into()));
        }
        self.captured
            .lock()
            .expect("fake anomaly emitter mutex poisoned")
            .push(features.clone());
        Ok(())
    }
}
