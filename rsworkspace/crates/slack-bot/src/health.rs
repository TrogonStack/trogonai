use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Bind a minimal HTTP server on `port` that responds to any request with
/// `200 ok`. Used as a liveness / readiness probe by Docker and Kubernetes.
pub async fn start_health_server(port: u16) {
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, addr = %addr, "Failed to bind health check port");
            return;
        }
    };
    tracing::info!(addr = %addr, "Health check server listening");

    loop {
        match listener.accept().await {
            Ok((mut socket, _)) => {
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = socket.read(&mut buf).await;
                    let response =
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok";
                    let _ = socket.write_all(response).await;
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "Health check accept error");
            }
        }
    }
}
