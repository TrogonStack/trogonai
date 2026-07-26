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
    let err = ServerInfoTimeoutError(Duration::from_secs(10));
    assert_eq!(err.to_string(), "timed out after 10s waiting for NATS server INFO");
}
