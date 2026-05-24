use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use a2a_bridge::{
    AppState, AsyncNatsAuthMintWire, AsyncNatsTokenGatewayUnary, AsyncNatsTokenTaskJetstream,
    AuthCalloutJsonMintClient, BridgeError, GatewayInboundPublisher, StubAuthCalloutClient,
    StubInboundGatewayPublish, StubTaskJetStreamPort, default_a2a_prefix, gateway_router,
};

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

    let state = match transport.as_str() {
        "stub" => bootstrap_stub_transport(&nats_url),
        "nats" => bootstrap_nats_transport(&nats_url).await?,
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

#[derive(Debug)]
enum BootstrapError {
    BadListenAddr,
    Listen(std::io::Error),
    Bridge(BridgeError),
    UnknownTransport(String),
    Serve(std::io::Error),
}

impl From<BridgeError> for BootstrapError {
    fn from(value: BridgeError) -> Self {
        Self::Bridge(value)
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadListenAddr => write!(f, "BRIDGE_LISTEN_ADDR must be a SocketAddr"),
            Self::Listen(e) => write!(f, "bind failed: {e}"),
            Self::Serve(e) => write!(f, "HTTP server failed: {e}"),
            Self::Bridge(inner) => write!(f, "{inner}"),
            Self::UnknownTransport(value) => {
                write!(f, "unknown A2A_BRIDGE_TRANSPORT (use stub or nats): {value}")
            }
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Listen(e) | Self::Serve(e) => Some(e),
            Self::Bridge(e) => Some(e),
            _ => None,
        }
    }
}

fn bootstrap_stub_transport(nats_url: &str) -> AppState {
    let auth_callout_url =
        env::var("AUTH_CALLOUT_NATS_URL").unwrap_or_else(|_| nats_url.trim().to_owned());
    tracing::warn!(
        transport = "stub",
        nats_url = %nats_url.trim(),
        auth_callout_url = %auth_callout_url,
        "using stub transports; outbound NATS/token paths no-op unless A2A_BRIDGE_TRANSPORT=nats"
    );
    AppState::new(
        Arc::new(StubAuthCalloutClient),
        Arc::new(StubInboundGatewayPublish),
        Arc::new(StubTaskJetStreamPort),
        default_a2a_prefix(),
    )
}

async fn bootstrap_nats_transport(nats_raw: &str) -> Result<AppState, BridgeError> {
    let servers = parse_nats_servers(nats_raw);
    if servers.is_empty() {
        return Err(BridgeError::NatsPublish(
            "NATS_URL/NATS bootstrap string produced zero servers".into(),
        ));
    }

    let connect_timeout =
        Duration::from_secs(parse_u64_env("BRIDGE_CONNECT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let mint_wire_timeout =
        Duration::from_secs(parse_u64_env("BRIDGE_AUTH_MINT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let gateway_rpc_timeout =
        Duration::from_secs(parse_u64_env("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS").unwrap_or(180).max(1));

    let client = async_nats::ConnectOptions::new()
        .connection_timeout(connect_timeout)
        .connect(servers.as_slice())
        .await
        .map_err(|e| BridgeError::NatsPublish(format!("anonymous NATS connect failed: {e}")))?;

    let client_arc = Arc::new(client);
    let wire = AsyncNatsAuthMintWire::new(client_arc.clone(), mint_wire_timeout);

    let mint_subject: Arc<str> = env::var("AUTH_CALLOUT_MINT_SUBJECT")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| Arc::from(s.into_boxed_str()))
        .unwrap_or_else(|| {
            Arc::from(AuthCalloutJsonMintClient::<AsyncNatsAuthMintWire>::default_mint_subject())
        });

    let auth_client = Arc::new(AuthCalloutJsonMintClient::new(Arc::new(wire), mint_subject));

    let unary = GatewayInboundPublisher::new(Arc::new(AsyncNatsTokenGatewayUnary::new(
        servers.clone(),
        gateway_rpc_timeout,
    )));

    tracing::info!(
        server_urls = ?servers.as_slice(),
        connect_timeout_secs = connect_timeout.as_secs(),
        mint_wire_secs = mint_wire_timeout.as_secs(),
        gateway_rpc_secs = gateway_rpc_timeout.as_secs(),
        "a2a-bridge NATS transports wired (JWT mint JSON + gateway unary + JetStream SSE intake)"
    );

    Ok(AppState::new(
        auth_client,
        Arc::new(unary),
        Arc::new(AsyncNatsTokenTaskJetstream::new(servers.clone(), gateway_rpc_timeout)),
        default_a2a_prefix(),
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
