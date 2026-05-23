use std::{env, net::SocketAddr, sync::Arc};

use a2a_bridge::{
    AppState, StubAuthCalloutClient, StubInboundGatewayPublish, StubTaskJetStreamPort,
    default_a2a_prefix, gateway_router,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::try_init().ok();

    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let auth_callout_url = env::var("AUTH_CALLOUT_NATS_URL").unwrap_or_else(|_| nats_url.clone());
    let listen: SocketAddr = env::var("BRIDGE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7443".to_string())
        .parse()
        .expect("BRIDGE_LISTEN_ADDR must parse as SocketAddr");

    tracing::info!(%listen, ?nats_url, ?auth_callout_url, "a2a-bridge env bound (NATS wired later)");

    let state = AppState::new(
        Arc::new(StubAuthCalloutClient),
        Arc::new(StubInboundGatewayPublish),
        Arc::new(StubTaskJetStreamPort),
        default_a2a_prefix(),
    );
    let router = gateway_router(state);

    let listener = tokio::net::TcpListener::bind(listen).await.expect("listen");
    axum::serve(listener, router).await.expect("serve");
}
