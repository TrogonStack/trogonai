use std::net::SocketAddr;

use prometheus::Registry;
use tracing::warn;

use super::background_loops::BackgroundLoops;

/// Serves the Prometheus scrape endpoint on `addr` until the loops are cancelled.
///
/// A listener that cannot start is logged and leaves the broker running without metrics.
pub(super) fn spawn(loops: &BackgroundLoops, addr: SocketAddr, registry: Registry) {
    let cancel = loops.cancel();
    loops.spawn(async move {
        let served = cancel
            .run_until_cancelled(trogon_sandbox_observability::serve_metrics(addr, registry))
            .await;
        if let Some(Err(error)) = served {
            warn!(error = %error, %addr, "metrics server exited");
        }
    });
}
