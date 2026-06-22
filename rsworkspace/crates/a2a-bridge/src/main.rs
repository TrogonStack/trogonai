use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use a2a_bridge::{
    AppState, AsyncNatsAuthMintWire, AsyncNatsTokenGatewayUnary, AsyncNatsTokenTaskJetstream,
    AuthCalloutJsonMintClient, BridgeError, BridgeTenantAccount, GatewayInboundPublisher, StubAuthCalloutClient,
    StubInboundGatewayPublish, StubTaskJetStreamPort, gateway_router,
};
use a2a_nats::{A2aPrefix, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::try_init().ok();
    if let Err(err) = run().await {
        tracing::error!(%err, "a2a-bridge exited with error");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), BootstrapError> {
    let transport = env::var("A2A_BRIDGE_TRANSPORT").unwrap_or_else(|_| "stub".into());

    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

    let listen: SocketAddr = env::var("BRIDGE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7443".to_string())
        .parse()
        .map_err(|_| BootstrapError::BadListenAddr)?;

    // Resolve A2A_PREFIX once before either transport so the bridge
    // shares its subject namespace with a2a-nats-http / -stdio /
    // -server — defaulting to "a2a" silently in production would make
    // any non-default deployment publish to a different gateway
    // subject and JetStream stream name than the rest of the stack.
    let prefix = resolve_a2a_prefix()?;

    let state = match transport.as_str() {
        "stub" => bootstrap_stub_transport(&nats_url, prefix),
        "nats" => bootstrap_nats_transport(&nats_url, prefix).await?,
        other => return Err(BootstrapError::UnknownTransport(other.into())),
    };

    tracing::info!(%listen, %transport, "a2a-bridge listening");

    let router = gateway_router(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(BootstrapError::Listen)?;
    axum::serve(listener, router).await.map_err(BootstrapError::Serve)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum BootstrapError {
    #[error("BRIDGE_LISTEN_ADDR must be a SocketAddr")]
    BadListenAddr,
    #[error("bind failed: {0}")]
    Listen(#[source] std::io::Error),
    #[error("{0}")]
    Bridge(#[from] BridgeError),
    #[error("unknown A2A_BRIDGE_TRANSPORT (use stub or nats): {0}")]
    UnknownTransport(String),
    #[error("HTTP server failed: {0}")]
    Serve(#[source] std::io::Error),
}

fn resolve_a2a_prefix() -> Result<A2aPrefix, BridgeError> {
    let raw = env::var(ENV_A2A_PREFIX).unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    A2aPrefix::new(raw).map_err(|e| BridgeError::NatsPublish(format!("{ENV_A2A_PREFIX} is invalid: {e}")))
}

fn bootstrap_stub_transport(nats_url: &str, prefix: A2aPrefix) -> AppState {
    let auth_callout_url = env::var("AUTH_CALLOUT_NATS_URL").unwrap_or_else(|_| nats_url.trim().to_owned());
    tracing::warn!(
        transport = "stub",
        nats_url = %nats_url.trim(),
        auth_callout_url = %auth_callout_url,
        prefix = %prefix.as_str(),
        "using stub transports; outbound NATS/token paths no-op unless A2A_BRIDGE_TRANSPORT=nats"
    );
    AppState::new(
        Arc::new(StubAuthCalloutClient),
        Arc::new(StubInboundGatewayPublish),
        Arc::new(StubTaskJetStreamPort),
        prefix,
    )
}

async fn bootstrap_nats_transport(nats_raw: &str, prefix: A2aPrefix) -> Result<AppState, BridgeError> {
    let servers = parse_nats_servers(nats_raw);
    if servers.is_empty() {
        return Err(BridgeError::NatsPublish(
            "NATS_URL/NATS bootstrap string produced zero servers".into(),
        ));
    }

    let connect_timeout = Duration::from_secs(parse_u64_env("BRIDGE_CONNECT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let mint_wire_timeout = Duration::from_secs(parse_u64_env("BRIDGE_AUTH_MINT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let gateway_rpc_timeout =
        Duration::from_secs(parse_u64_env("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS").unwrap_or(180).max(1));

    let mut connect_opts = async_nats::ConnectOptions::new().connection_timeout(connect_timeout);
    if let (Ok(user), Ok(password)) = (env::var("NATS_USER"), env::var("NATS_PASSWORD")) {
        connect_opts = connect_opts.user_and_password(user, password);
    }
    let client = connect_opts
        .connect(servers.as_slice())
        .await
        .map_err(|e| BridgeError::NatsPublish(format!("NATS connect failed: {e}")))?;

    let client_arc = Arc::new(client);
    let wire = AsyncNatsAuthMintWire::new(client_arc.clone(), mint_wire_timeout);

    let mint_subject: Arc<str> = env::var("AUTH_CALLOUT_MINT_SUBJECT")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| Arc::from(s.into_boxed_str()))
        .unwrap_or_else(|| Arc::from(AuthCalloutJsonMintClient::<AsyncNatsAuthMintWire>::default_mint_subject()));

    let tenant_account = match env::var("BRIDGE_TENANT_ACCOUNT") {
        Ok(raw) if !raw.trim().is_empty() => Some(BridgeTenantAccount::new(raw)?),
        _ => None,
    };

    let auth_client = Arc::new(AuthCalloutJsonMintClient::with_tenant_account(
        Arc::new(wire),
        mint_subject,
        tenant_account,
    ));

    let unary = GatewayInboundPublisher::new(Arc::new(AsyncNatsTokenGatewayUnary::new(
        servers.clone(),
        gateway_rpc_timeout,
    )));

    tracing::info!(
        server_urls = ?servers.as_slice(),
        connect_timeout_secs = connect_timeout.as_secs(),
        mint_wire_secs = mint_wire_timeout.as_secs(),
        gateway_rpc_secs = gateway_rpc_timeout.as_secs(),
        prefix = %prefix.as_str(),
        "a2a-bridge NATS transports wired (JWT mint JSON + gateway unary + JetStream SSE intake)"
    );

    Ok(AppState::new(
        auth_client,
        Arc::new(unary),
        Arc::new(AsyncNatsTokenTaskJetstream::new(servers.clone(), gateway_rpc_timeout)),
        prefix,
    ))
}

fn parse_nats_servers(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with("nats://") || s.starts_with("tls://") {
                s.to_owned()
            } else {
                format!("nats://{s}")
            }
        })
        .collect()
}

fn parse_u64_env(key: &str) -> Option<u64> {
    env::var(key).ok()?.trim().parse().ok()
}
