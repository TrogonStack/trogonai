use std::future::Future;
use std::time::Duration;

/// Bounds a test body so a stalled fake can never hang the suite.
pub(super) async fn within<T>(
    body: impl Future<Output = Result<T, Box<dyn std::error::Error>>>,
) -> Result<T, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(20), body).await?
}
