use super::shutdown_signal;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn shutdown_signal_waits_for_signal() {
    let result = timeout(Duration::from_millis(10), shutdown_signal()).await;
    assert!(result.is_err());
}
