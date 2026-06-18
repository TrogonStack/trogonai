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
    let poll_interval = poll_interval.max(Duration::from_millis(1));

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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StubSource {
        ready_after: usize,
        info: ServerInfo,
        calls: AtomicUsize,
    }

    impl StubSource {
        fn ready_after(ready_after: usize, max_payload: usize) -> Self {
            Self {
                ready_after,
                info: ServerInfo {
                    max_payload,
                    ..Default::default()
                },
                calls: AtomicUsize::new(0),
            }
        }

        fn never() -> Self {
            Self::ready_after(usize::MAX, 0)
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ServerInfoSource for StubSource {
        fn try_server_info(&self) -> Option<ServerInfo> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            (n >= self.ready_after).then(|| self.info.clone())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn returns_immediately_when_info_already_present() {
        let source = StubSource::ready_after(0, 1_048_576);

        let info = wait_for_server_info(&source, Duration::from_secs(10), Duration::from_millis(50))
            .await
            .expect("info should be available immediately");

        assert_eq!(info.max_payload, 1_048_576);
        assert_eq!(source.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn returns_once_info_arrives_after_some_polls() {
        let source = StubSource::ready_after(3, 2_097_152);

        let info = wait_for_server_info(&source, Duration::from_secs(10), Duration::from_millis(50))
            .await
            .expect("info should become available before timeout");

        assert_eq!(info.max_payload, 2_097_152);
        assert_eq!(source.calls(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_when_info_never_arrives() {
        let source = StubSource::never();

        let err = wait_for_server_info(&source, Duration::from_millis(200), Duration::from_millis(50))
            .await
            .expect_err("expected a timeout");

        assert_eq!(err.0, Duration::from_millis(200));
        assert!(source.calls() >= 1);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_poll_interval_does_not_busy_loop() {
        let source = StubSource::ready_after(2, 4_096);

        let info = wait_for_server_info(&source, Duration::from_secs(1), Duration::ZERO)
            .await
            .expect("info should become available with a clamped poll interval");

        assert_eq!(info.max_payload, 4_096);
    }

    #[test]
    fn timeout_error_displays_duration() {
        let err = ServerInfoTimeout(Duration::from_secs(10));
        assert!(err.to_string().contains("waiting for NATS server INFO"));
    }
}
