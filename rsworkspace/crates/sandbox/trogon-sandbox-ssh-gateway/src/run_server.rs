use std::net::SocketAddr;
use std::sync::Arc;

use russh::keys::PrivateKey;
use russh::server::Config;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::constants::{AUTH_REJECTION_TIME, INACTIVITY_TIMEOUT};
use crate::ssh_gateway_handler::SshGatewayHandler;
use crate::{GatewayState, ServeError};

/// Serves SSH connections from `listener` until `shutdown` fires. At most `max_connections` are served at once;
/// further clients wait in the accept queue.
pub async fn run_server(
    listener: TcpListener,
    host_key: PrivateKey,
    state: Arc<GatewayState>,
    shutdown: CancellationToken,
) -> Result<(), ServeError> {
    let config = Arc::new(Config {
        inactivity_timeout: Some(INACTIVITY_TIMEOUT),
        auth_rejection_time: AUTH_REJECTION_TIME,
        keys: vec![host_key],
        ..Default::default()
    });
    info!(listen = ?listener.local_addr().ok(), "trogon-sandbox-ssh-gateway listening");
    loop {
        let (stream, _peer, permit) = tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            accepted = accept(&listener, &state.connection_slots) => accepted?,
        };
        let config = Arc::clone(&config);
        let handler = SshGatewayHandler::new(Arc::clone(&state));
        tokio::spawn(async move {
            let _permit = permit;
            let ended = serve_connection(config, stream, handler).await;
            drop(ended.inspect_err(|error| debug!(%error, "ssh connection ended with an error")));
        });
    }
}

async fn accept(
    listener: &TcpListener,
    slots: &Arc<Semaphore>,
) -> Result<(TcpStream, SocketAddr, OwnedSemaphorePermit), ServeError> {
    let permit = Arc::clone(slots).acquire_owned().await?;
    let (stream, peer) = listener.accept().await?;
    Ok((stream, peer, permit))
}

async fn serve_connection(
    config: Arc<russh::server::Config>,
    stream: tokio::net::TcpStream,
    handler: SshGatewayHandler,
) -> Result<(), crate::GatewayError> {
    russh::server::run_stream(config, stream, handler).await?.await
}
