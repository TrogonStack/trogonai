use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use tokio::sync::mpsc::Receiver;
use tracing::warn;
use trogon_sandbox_contract::{FsViolation, LeaseId};
use trogon_sandbox_egress_proxy::{ViolationRateLimit, ViolationRateLimiter, ViolationSink};
use trogon_sandbox_protocol::subjects::fs_violation;

use super::FsDenial;

pub struct FsViolationReporter {
    sink: Arc<dyn ViolationSink>,
    limiter: Mutex<ViolationRateLimiter>,
}

impl FsViolationReporter {
    pub fn new(sink: Arc<dyn ViolationSink>, limit: ViolationRateLimit) -> Self {
        Self {
            sink,
            limiter: Mutex::new(ViolationRateLimiter::new(limit)),
        }
    }

    pub async fn report(&self, lease: &LeaseId, denial: FsDenial, at: SystemTime) {
        let admitted = self
            .limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit(lease, Instant::now());
        if !admitted {
            return;
        }
        let violation = FsViolation {
            lease_id: lease.clone(),
            path: denial.path,
            operation: denial.operation,
            at,
        };
        let payload = match serde_json::to_vec(&violation) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(%error, "failed to serialize filesystem violation");
                return;
            }
        };
        if let Err(error) = self.sink.publish(fs_violation(lease), payload).await {
            warn!(%error, "failed to publish filesystem violation");
        }
    }

    /// Publishes every denial of `lease` until all senders are gone.
    pub fn drain(self: &Arc<Self>, lease: LeaseId, mut denials: Receiver<FsDenial>) {
        let reporter = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(denial) = denials.recv().await {
                reporter.report(&lease, denial, SystemTime::now()).await;
            }
        });
    }
}
