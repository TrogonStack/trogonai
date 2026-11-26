//! Announcing a streamed command's exit on
//! `sandbox.evidence.{lease_id}.command_exit.{execution_id}`, shared by the
//! workers whose guest agent mints execution ids.
//!
//! A detached command replies `Started` before it runs, so nothing on the
//! request path sees it exit. [`await_detached_exit`] polls the agent until it
//! does, which lets the worker publish the marker a log follower is waiting on.

use std::future::{Future, IntoFuture};
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use trogon_nats::jetstream::traits::JetStreamPublisher;
use trogon_sandbox_contract::{CommandExitMarker, ExecutionId, LeaseId, PollCommandResponse};

use crate::command_exit_error::CommandExitError;
use crate::detached_exit::DetachedExit;
use crate::subjects;

/// How often a detached command's watcher asks the agent whether it exited.
pub const DETACHED_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Consecutive failed polls after which the watcher stops and reports the
/// exit as unknown. The agent going quiet this long means the sandbox is gone,
/// and a follower is better told so than left waiting.
pub const DETACHED_EXIT_POLL_FAILURES: u32 = 20;

/// Polls `poll` every `interval` until the command leaves `Running`.
///
/// A failed poll is retried; `max_failures` failures in a row end the watch
/// as [`DetachedExit::Unknown`].
pub async fn await_detached_exit<F, Fut, E>(mut poll: F, interval: Duration, max_failures: u32) -> DetachedExit
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<PollCommandResponse, E>>,
{
    let mut failures = 0;
    loop {
        match poll().await {
            Ok(PollCommandResponse::Running) => failures = 0,
            Ok(PollCommandResponse::Completed(output)) => {
                return DetachedExit::Exited(output.exit_code);
            }
            Ok(PollCommandResponse::NotFound | PollCommandResponse::Denied(_)) => {
                return DetachedExit::Unknown;
            }
            Err(_) => {
                failures += 1;
                if failures >= max_failures {
                    return DetachedExit::Unknown;
                }
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Publishes the exit marker for `execution_id` on `lease_id`'s evidence
/// stream. Call it after the command's last chunk was published, since a
/// follower stops reading at the marker.
pub async fn publish_command_exit<J>(
    js: &J,
    lease_id: &LeaseId,
    execution_id: &ExecutionId,
    exit_code: Option<i32>,
) -> Result<(), CommandExitError>
where
    J: JetStreamPublisher,
{
    let marker = CommandExitMarker {
        execution_id: execution_id.clone(),
        exit_code,
    };
    let payload = serde_json::to_vec(&marker)?;
    let ack = js
        .publish_with_headers(
            subjects::command_exit_subject(lease_id, execution_id),
            HeaderMap::new(),
            Bytes::from(payload),
        )
        .await
        .map_err(|error| CommandExitError::Publish(Box::new(error)))?;
    ack.into_future()
        .await
        .map_err(|error| CommandExitError::Publish(Box::new(error)))?;
    Ok(())
}
