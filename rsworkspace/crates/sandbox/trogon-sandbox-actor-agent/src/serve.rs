use std::fmt::Debug;

use axum::Router;
use tokio_util::sync::CancellationToken;

/// Serves `app` on `listener` until `cancel` fires, then lets in-flight requests finish.
pub async fn serve<L>(listener: L, app: Router, cancel: CancellationToken) -> std::io::Result<()>
where
    L: axum::serve::Listener,
    L::Addr: Debug,
{
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
}

/// Serves `app` on an `AF_VSOCK` port, any CID. Used inside Firecracker guests, where the host reaches the agent
/// through the hybrid vsock Unix socket.
#[cfg(target_os = "linux")]
pub async fn serve_vsock(port: u32, app: Router, cancel: CancellationToken) -> std::io::Result<()> {
    let bound = tokio_vsock::VsockListener::bind(tokio_vsock::VsockAddr::new(tokio_vsock::VMADDR_CID_ANY, port));
    serve_bound(bound, app, cancel).await
}

/// Serves on a listener that may have failed to bind, reporting the bind failure as the result.
#[cfg(any(target_os = "linux", test))]
async fn serve_bound<L>(bound: std::io::Result<L>, app: Router, cancel: CancellationToken) -> std::io::Result<()>
where
    L: axum::serve::Listener,
    L::Addr: Debug,
{
    serve(bound?, app, cancel).await
}

/// Vsock exists only on Linux, so anywhere else the request is refused rather than silently served over TCP.
#[cfg(not(target_os = "linux"))]
pub async fn serve_vsock(_port: u32, _app: Router, _cancel: CancellationToken) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "vsock listening is only supported on Linux",
    ))
}

#[cfg(test)]
mod tests;
