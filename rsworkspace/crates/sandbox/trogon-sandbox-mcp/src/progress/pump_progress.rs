use std::future::Future;

use futures::{Stream, StreamExt};
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use trogon_sandbox_client::{ClientError, LogFollowEvent};

use super::FollowEnd;
use super::progress_for_chunk::progress_for_chunk;

/// Sends `events` as progress notifications through `send`, one per chunk, until the follow ends. `send` returns
/// `false` when the client is gone, which stops the pump.
pub(crate) async fn pump_progress<S, F, Fut>(mut events: S, token: &ProgressToken, mut send: F) -> FollowEnd
where
    S: Stream<Item = Result<LogFollowEvent, ClientError>> + Unpin,
    F: FnMut(ProgressNotificationParam) -> Fut,
    Fut: Future<Output = bool>,
{
    let mut count = 0u64;
    while let Some(item) = events.next().await {
        match item {
            Ok(LogFollowEvent::Chunk(chunk)) => {
                count += 1;
                if !send(progress_for_chunk(token, count, &chunk)).await {
                    return FollowEnd::Interrupted;
                }
            }
            Ok(LogFollowEvent::End { .. }) => return FollowEnd::Exited,
            Ok(LogFollowEvent::Unsupported) => return FollowEnd::Unsupported,
            // Progress is best effort: a retry or a dropped span of output only changes what the final result
            // carries.
            Ok(LogFollowEvent::Reconnecting { .. } | LogFollowEvent::ResyncRequired { .. }) => {}
            Err(_) => return FollowEnd::Interrupted,
        }
    }
    FollowEnd::Interrupted
}

#[cfg(test)]
mod tests;
