use crate::constants::MIN_SERVER_INFO_POLL_INTERVAL;
use async_nats::{Client, ServerInfo};
use std::time::Duration;

/// Source of the most recently observed NATS server `INFO`.
///
/// `async_nats` seeds its `INFO` watch channel with `None` and only populates
/// it once the first `INFO` frame arrives. When `retry_on_initial_connect` is
/// enabled, [`connect`](crate::connect) returns the [`Client`] before that
/// frame has been received, so the value is observably `None` for a short
/// window after connecting. This trait abstracts that read so readiness can be
/// awaited and tested without a live server.
pub trait ServerInfoSource {
    /// Returns the last observed server `INFO`, or `None` if none has arrived yet.
    fn try_server_info(&self) -> Option<ServerInfo>;
}

impl ServerInfoSource for Client {
    fn try_server_info(&self) -> Option<ServerInfo> {
        Client::try_server_info(self)
    }
}

/// The server `INFO` did not arrive within the allotted time.
#[derive(Debug, thiserror::Error)]
#[error("timed out after {0:?} waiting for NATS server INFO")]
pub struct ServerInfoTimeout(pub Duration);

/// Waits until the server `INFO` is available, polling every `poll_interval`.
///
/// `async_nats` does not expose the `INFO` watch channel, so readiness is
/// observed by polling [`ServerInfoSource::try_server_info`]. Returns
/// [`ServerInfoTimeout`] if no `INFO` arrives within `timeout`.
///
/// `poll_interval` must be non-zero; a zero interval would busy-loop and is
/// clamped to 1ms.
pub async fn wait_for_server_info<S>(
    source: &S,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<ServerInfo, ServerInfoTimeout>
where
    S: ServerInfoSource + ?Sized,
{
    let poll_interval = poll_interval.max(MIN_SERVER_INFO_POLL_INTERVAL);

    tokio::time::timeout(timeout, async {
        loop {
            if let Some(info) = source.try_server_info() {
                return info;
            }
            tokio::time::sleep(poll_interval).await;
        }
    })
    .await
    .map_err(|_| ServerInfoTimeout(timeout))
}

#[cfg(test)]
mod tests;
