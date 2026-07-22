//! Canonical cancellation transport: asks the runner to stop an in-flight operation via
//! the ACP cancel notification, used by the kernel's `cancel_operation` flow
//! (cambio-modelo.md § Cancellation and Abort Semantics). It is `Send + Sync` because it
//! talks to the runner over `async_nats::Client`, not the `!Send` ACP `Bridge`.

use async_trait::async_trait;
use bytes::Bytes;
use trogonai_switching::{RunnerCancelOutcome, RunnerCancellation, SwitchingError};

/// [`RunnerCancellation`] that publishes the ACP cancel notification to a runner over NATS
/// (the same `{prefix}.session.{id}.agent.cancel` subject the live CLI uses).
pub struct SessionRunnerCancellation {
    nats: async_nats::Client,
    prefix: String,
    session_id: String,
}

impl SessionRunnerCancellation {
    pub fn new(nats: async_nats::Client, prefix: String, session_id: String) -> Self {
        Self {
            nats,
            prefix,
            session_id,
        }
    }
}

#[async_trait]
impl RunnerCancellation for SessionRunnerCancellation {
    async fn cancel(&self) -> Result<RunnerCancelOutcome, SwitchingError> {
        let subject = format!("{}.session.{}.agent.cancel", self.prefix, self.session_id);
        let payload = serde_json::to_vec(&serde_json::json!({ "sessionId": self.session_id }))
            .map_err(|err| SwitchingError::CancelFailed { detail: err.to_string() })?;
        self.nats
            .publish(subject, Bytes::from(payload))
            .await
            .map_err(|err| SwitchingError::CancelFailed { detail: err.to_string() })?;
        // The ACP cancel is a fire-and-forget notification: the runner stops its loop. We
        // report `Cancelled` best-effort; if a tool had already completed, the tool's
        // receipt/`requires_reconciliation` state (derived from the snapshot) governs the
        // canonical outcome rather than this transport.
        Ok(RunnerCancelOutcome::Cancelled)
    }
}
