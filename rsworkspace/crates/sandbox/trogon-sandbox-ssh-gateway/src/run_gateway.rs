use std::sync::Arc;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_auth::NatsKvApiKeyStore;
use trogon_sandbox_observability::SSH_GATEWAY;
use trogon_sandbox_protocol::DispatchSigner;
use trogon_std::env::SystemEnv;

use crate::{
    GatewayArgs, GatewayConfig, GatewayRunError, GatewayState, NatsSshTunnelBackend, SessionKeyMint,
    load_or_generate_host_key, run_server,
};

/// Connects the gateway to NATS and serves SSH until `shutdown` resolves.
pub async fn run_gateway<F>(args: GatewayArgs, shutdown: F) -> Result<(), GatewayRunError>
where
    F: Future<Output = Result<(), GatewayRunError>>,
{
    let (_tracing, _registry) =
        trogon_sandbox_observability::bootstrap("trogon-sandbox-ssh-gateway", SSH_GATEWAY, &args.observability)?;

    let nats = async_nats::connect(&args.nats_url).await?;
    info!(url = %args.nats_url, "connected to NATS");

    let api_keys = Arc::new(
        NatsKvApiKeyStore::new_with_bucket(async_nats::jetstream::new(nats.clone()), &args.api_key_bucket).await?,
    );

    let mut backend = NatsSshTunnelBackend::new(nats.clone());
    match DispatchSigner::from_env(&SystemEnv)? {
        Some(signer) => backend = backend.with_signer(Arc::new(signer)),
        None => warn!("no dispatch signing key; ssh attach and detach are sent unsigned"),
    }

    let broker = match &args.credential_broker_url {
        Some(url) => async_nats::connect(url).await.ok(),
        None => Some(nats),
    };
    let key_mint = broker
        .map(|client| SessionKeyMint::new(client, args.ssh_gateway_tenant.clone(), args.ssh_pull_secret_id.clone()))
        .map(Arc::new);
    if key_mint.is_none() {
        warn!("credential broker unavailable; session tokens will not be registered");
    }

    let host_key = load_or_generate_host_key(&args.host_key_path)?;
    let listener = TcpListener::bind(args.listen).await.map_err(GatewayRunError::Bind)?;
    let state = Arc::new(GatewayState::new(GatewayConfig {
        api_keys,
        tunnel: Arc::new(backend),
        key_mint,
        max_connections: args.max_connections,
    }));

    let cancel = CancellationToken::new();
    let serve = run_server(listener, host_key, state, cancel.clone());
    tokio::pin!(serve);
    tokio::select! {
        result = &mut serve => result?,
        outcome = shutdown => {
            cancel.cancel();
            outcome?;
            serve.await?;
        }
    }
    Ok(())
}

/// Resolves when Ctrl+C is received.
pub async fn shutdown_signal() -> Result<(), GatewayRunError> {
    tokio::signal::ctrl_c().await.map_err(GatewayRunError::Signal)
}
