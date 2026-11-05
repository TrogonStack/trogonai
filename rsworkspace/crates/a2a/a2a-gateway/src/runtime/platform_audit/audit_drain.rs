use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::DecisionAudit;

/// The audit sink's background work for as long as the gateway serves.
///
/// It settles only once the ingress loop has returned. The shutdown signal alone
/// is too early: the dispatch in progress when it fires still records its
/// decision, and that record has to reach [`DecisionAudit::run`] before its last
/// flush.
pub struct AuditDrain {
    ingress_stopped: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl AuditDrain {
    /// Spawns [`DecisionAudit::run`] for `audit`, or nothing when audit
    /// publishing is off.
    pub fn start(audit: Option<Arc<dyn DecisionAudit>>) -> Self {
        let ingress_stopped = CancellationToken::new();
        let task = audit.map(|audit| {
            let stop = ingress_stopped.clone();
            tokio::spawn(async move { audit.run(stop).await })
        });
        Self { ingress_stopped, task }
    }

    /// Tells the sink no more decisions are coming and waits for its last
    /// flush. Call it only after the ingress loop has returned.
    pub async fn settle(self) {
        self.ingress_stopped.cancel();
        if let Some(task) = self.task
            && let Err(error) = task.await
        {
            warn!(%error, "gateway audit drain task ended abnormally");
        }
    }
}

#[cfg(test)]
mod tests;
