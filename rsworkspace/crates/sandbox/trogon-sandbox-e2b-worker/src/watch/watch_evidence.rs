use async_nats::jetstream::context::PublishError;
use async_nats::jetstream::{self, publish::PublishAck};
use tracing::warn;
use trogon_sandbox_contract::{FileWatchOverflowEvidence, LeaseId};
use trogon_sandbox_protocol::subjects::file_watch_overflow_evidence;

use super::watches::Watches;

async fn publish_acked(js: &jetstream::Context, subject: &str, payload: Vec<u8>) -> Result<PublishAck, PublishError> {
    js.publish(subject.to_owned(), payload.into()).await?.await
}

impl Watches {
    /// Puts a watch's overflow on the lease's evidence timeline, once per new loss. Best effort: a failed publish is
    /// logged and never fails the poll.
    pub(super) async fn publish_overflow(&self, lease: &LeaseId, watch_id: &str) {
        let Some(js) = &self.evidence else { return };
        let Some(watch) = self
            .leases()
            .get(lease.as_str())
            .and_then(|held| held.get(watch_id))
            .cloned()
        else {
            return;
        };
        let dropped_through = {
            let mut buffer = watch.buffer.lock().await;
            if buffer.dropped_through <= buffer.reported_through {
                return;
            }
            buffer.reported_through = buffer.dropped_through;
            buffer.dropped_through
        };
        let evidence = FileWatchOverflowEvidence {
            watch_id: watch_id.to_owned(),
            dropped_through,
        };
        let subject = file_watch_overflow_evidence(lease);
        if let Ok(payload) = serde_json::to_vec(&evidence)
            && let Err(error) = publish_acked(js, &subject, payload).await
        {
            warn!(subject = %subject, error = %error, "failed to publish file watch overflow");
        }
    }
}
