use std::future::Future;

use axum::Router;
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;
use tokio::net::TcpListener;

use crate::{GatewayError, TlsFiles};

/// Serves `app` over TLS on an already bound listener until `shutdown` resolves.
pub async fn serve_tls_with_shutdown<F>(
    listener: TcpListener,
    tls: &TlsFiles,
    app: Router,
    shutdown: F,
) -> Result<(), GatewayError>
where
    F: Future<Output = Result<(), GatewayError>> + Send + 'static,
{
    trogon_std::tls::install_default_crypto_provider();
    let config = RustlsConfig::from_pem_file(tls.cert(), tls.key())
        .await
        .map_err(GatewayError::Tls)?;
    let listener = listener.into_std().map_err(GatewayError::Bind)?;
    let handle = Handle::new();
    let stopper = handle.clone();
    let stopped = tokio::spawn(async move {
        let outcome = shutdown.await;
        stopper.graceful_shutdown(None);
        outcome
    });
    axum_server::from_tcp_rustls(listener, config)
        .map_err(GatewayError::Bind)?
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .map_err(GatewayError::Serve)?;
    stopped.await.unwrap_or(Ok(()))
}
