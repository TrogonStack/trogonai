#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use {
    a2a_auth_callout::XkeyPublic,
    a2a_bridge::{
        AppState, AsyncNatsAuthMintWire, AsyncNatsTokenGatewayUnary, AsyncNatsTokenTaskJetstream,
        AuthCalloutJsonMintClient, BridgeError, BridgeTenantAccount, CallerConnectTimeout, GatewayInboundPublisher,
        GatewayRpcTimeout, StubAuthCalloutClient, StubInboundGatewayPublish, StubTaskJetStreamPort, gateway_router,
    },
    a2a_nats::{A2aPrefix, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX},
    std::{future::Future, net::SocketAddr, sync::Arc, time::Duration},
    trogon_nats::NatsConfig,
    trogon_std::env::{ReadEnv, SystemEnv},
};

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() {
    tracing_subscriber::fmt::try_init().ok();
    if let Err(err) = run(&SystemEnv, std::future::pending()).await {
        tracing::error!(%err, "a2a-bridge exited with error");
        std::process::exit(1);
    }
}

async fn run(env: &impl ReadEnv, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<(), BootstrapError> {
    let transport = env.var("A2A_BRIDGE_TRANSPORT").unwrap_or_else(|_| "stub".into());

    let nats = NatsConfig::from_env(env);

    let listen: SocketAddr = env
        .var("BRIDGE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7443".to_string())
        .parse()
        .map_err(|_| BootstrapError::BadListenAddr)?;

    // Resolve A2A_PREFIX once before either transport so the bridge
    // shares its subject namespace with a2a-nats-http / -stdio /
    // -server — defaulting to "a2a" silently in production would make
    // any non-default deployment publish to a different gateway
    // subject and JetStream stream name than the rest of the stack.
    let prefix = resolve_a2a_prefix(env)?;

    let state = match transport.as_str() {
        "stub" => bootstrap_stub_transport(env, &nats, prefix),
        "nats" => bootstrap_nats_transport(env, &nats, prefix).await?,
        other => return Err(BootstrapError::UnknownTransport(other.into())),
    };

    tracing::info!(%listen, %transport, "a2a-bridge listening");

    let router = gateway_router(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(BootstrapError::Listen)?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(BootstrapError::Serve)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum BootstrapError {
    #[error("BRIDGE_LISTEN_ADDR must be a SocketAddr")]
    BadListenAddr,
    #[error("NATS_URL does not name any server")]
    NoNatsServers,
    #[error("NATS connect failed: {0}")]
    NatsConnect(#[source] trogon_nats::ConnectError),
    #[error("AUTH_CALLOUT_XKEY_PUBLIC is not a curve public key: {0}")]
    CalloutXkey(#[source] a2a_auth_callout::AuthCalloutError),
    #[error("bind failed: {0}")]
    Listen(#[source] std::io::Error),
    #[error("{0}")]
    Bridge(#[from] a2a_bridge::BridgeError),
    #[error("unknown A2A_BRIDGE_TRANSPORT (use stub or nats): {0}")]
    UnknownTransport(String),
    #[error("HTTP server failed: {0}")]
    Serve(#[source] std::io::Error),
}

fn resolve_a2a_prefix(env: &impl ReadEnv) -> Result<A2aPrefix, BridgeError> {
    let raw = env
        .var(ENV_A2A_PREFIX)
        .unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    A2aPrefix::new(raw).map_err(|e| BridgeError::NatsPublish(format!("{ENV_A2A_PREFIX} is invalid: {e}")))
}

fn resolve_callout_xkey(env: &impl ReadEnv) -> Result<Option<XkeyPublic>, BootstrapError> {
    match env.var("AUTH_CALLOUT_XKEY_PUBLIC") {
        Ok(raw) if !raw.trim().is_empty() => XkeyPublic::parse(raw).map(Some).map_err(BootstrapError::CalloutXkey),
        _ => Ok(None),
    }
}

fn bootstrap_stub_transport(env: &impl ReadEnv, nats: &NatsConfig, prefix: A2aPrefix) -> AppState {
    let nats_url = nats.servers.join(",");
    let auth_callout_url = env.var("AUTH_CALLOUT_NATS_URL").unwrap_or_else(|_| nats_url.clone());
    tracing::warn!(
        transport = "stub",
        nats_url = %nats_url,
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

async fn bootstrap_nats_transport(
    env: &impl ReadEnv,
    nats: &NatsConfig,
    prefix: A2aPrefix,
) -> Result<AppState, BootstrapError> {
    if nats.servers.is_empty() {
        return Err(BootstrapError::NoNatsServers);
    }

    let connect_timeout = Duration::from_secs(parse_u64_env(env, "BRIDGE_CONNECT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let mint_wire_timeout =
        Duration::from_secs(parse_u64_env(env, "BRIDGE_AUTH_MINT_TIMEOUT_SECS").unwrap_or(30).max(1));
    let gateway_rpc_timeout = Duration::from_secs(
        parse_u64_env(env, "BRIDGE_GATEWAY_RPC_TIMEOUT_SECS")
            .unwrap_or(180)
            .max(1),
    );
    let callout_xkey = resolve_callout_xkey(env)?;

    let client = trogon_nats::connect(nats, connect_timeout)
        .await
        .map_err(BootstrapError::NatsConnect)?;

    let client_arc = Arc::new(client);
    let wire = AsyncNatsAuthMintWire::new(client_arc.clone(), mint_wire_timeout);

    let mint_subject: Arc<str> = env
        .var("AUTH_CALLOUT_MINT_SUBJECT")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| Arc::from(s.into_boxed_str()))
        .unwrap_or_else(|| Arc::from(AuthCalloutJsonMintClient::<AsyncNatsAuthMintWire>::default_mint_subject()));

    let tenant_account = match env.var("BRIDGE_TENANT_ACCOUNT") {
        Ok(raw) if !raw.trim().is_empty() => Some(BridgeTenantAccount::new(raw)?),
        _ => None,
    };

    let auth_client = AuthCalloutJsonMintClient::with_tenant_account(Arc::new(wire), mint_subject, tenant_account);
    let auth_client = Arc::new(match callout_xkey {
        Some(callout_xkey) => auth_client.sealing_to(callout_xkey),
        None => {
            tracing::warn!(
                env_var = "AUTH_CALLOUT_XKEY_PUBLIC",
                "no callout xkey is configured, so every caller credential mint is refused"
            );
            auth_client
        }
    });

    let caller_connect_timeout = CallerConnectTimeout::new(connect_timeout);
    let unary = GatewayInboundPublisher::new(Arc::new(AsyncNatsTokenGatewayUnary::new(
        nats.clone(),
        caller_connect_timeout,
        GatewayRpcTimeout::new(gateway_rpc_timeout),
    )));

    tracing::info!(
        server_urls = ?nats.servers.as_slice(),
        connect_timeout_secs = connect_timeout.as_secs(),
        mint_wire_secs = mint_wire_timeout.as_secs(),
        gateway_rpc_secs = gateway_rpc_timeout.as_secs(),
        prefix = %prefix.as_str(),
        "a2a-bridge NATS transports wired (JWT mint JSON + gateway unary + JetStream SSE intake)"
    );

    Ok(AppState::new(
        auth_client,
        Arc::new(unary),
        Arc::new(AsyncNatsTokenTaskJetstream::new(nats.clone(), caller_connect_timeout)),
        prefix,
    ))
}

fn parse_u64_env(env: &impl ReadEnv, key: &str) -> Option<u64> {
    env.var(key).ok()?.trim().parse().ok()
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
