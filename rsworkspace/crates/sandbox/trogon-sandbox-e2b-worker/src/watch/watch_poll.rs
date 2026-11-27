use std::time::Duration;

use tokio::time::Instant;
use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpResponse};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::watch_outcome::WatchOutcome;
use super::watches::Watches;
use crate::api::ApiError;
use crate::envd::EnvdTarget;

impl Watches {
    pub(super) async fn poll(
        &self,
        lease: &str,
        target: &EnvdTarget,
        watch_id: &str,
        after_seq: u64,
        wait_ms: Option<u32>,
    ) -> WatchOutcome {
        let unknown = || {
            FileOpError::failed(
                FileOpErrorKind::UnknownWatch,
                format!("watch {watch_id} is not live on this lease"),
            )
        };
        let Some(watch) = self.leases().get(lease).and_then(|held| held.get(watch_id)).cloned() else {
            return Err(unknown());
        };
        let wait = Duration::from_millis(u64::from(wait_ms.unwrap_or(0).min(FileOp::WATCH_MAX_WAIT_MS)));
        let deadline = Instant::now() + wait;
        loop {
            {
                let mut buffer = watch.buffer.lock().await;
                buffer.touched = Instant::now();
                match self.envd.watcher_events(target, &watch.user, &watch.envd_id).await {
                    Ok(batch) => buffer.push(&watch.root, batch.events, batch.dropped),
                    Err(ApiError::NotFound(_)) => {
                        // envd lost the watcher (for example the sandbox restarted).
                        drop(buffer);
                        self.forget(lease, watch_id);
                        return Err(unknown());
                    }
                    Err(error) => return Err(api_failure(&error, &watch.root)),
                }
                if let Some(response) = buffer.ready(after_seq) {
                    return Ok(response);
                }
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    return Ok(FileOpResponse::WatchEvents {
                        events: Vec::new(),
                        next_seq: after_seq,
                        overflowed: buffer.overflowed_past(after_seq),
                    });
                }
            }
            let pause = self
                .poll_interval
                .min(deadline.saturating_duration_since(Instant::now()));
            tokio::select! {
                biased;
                _ = watch.gone.cancelled() => return Err(unknown()),
                _ = tokio::time::sleep(pause) => {}
            }
        }
    }
}
