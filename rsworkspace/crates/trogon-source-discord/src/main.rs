#[cfg(not(coverage))]
use {
    acp_telemetry::ServiceName,
    futures_core::Stream,
    std::future::poll_fn,
    std::pin::Pin,
    std::sync::Arc,
    tracing::info,
    tracing::warn,
    trogon_nats::connect,
    trogon_nats::jetstream::NatsJetStreamClient,
    trogon_source_discord::DiscordConfig,
    trogon_source_discord::config::SourceMode,
    trogon_source_discord::constants::DEFAULT_NATS_CONNECT_TIMEOUT,
    trogon_source_discord::gateway::GatewayBridge,
    trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
    twilight_gateway::{Message, Shard, ShardId},
};

#[cfg(not(coverage))]
async fn run_gateway(
    js: NatsJetStreamClient,
    config: &DiscordConfig,
    bot_token: &str,
    intents: twilight_model::gateway::Intents,
) {
    info!("mode: gateway");

    let bridge = GatewayBridge::new(
        Arc::new(js),
        config.subject_prefix.clone(),
        config.nats_ack_timeout,
    );

    let health_addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    let health_app = axum::Router::new().route(
        "/health",
        axum::routing::get(|| async { axum::http::StatusCode::OK }),
    );
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(health_addr)
            .await
            .expect("failed to bind health port");
        info!(addr = %health_addr, "health endpoint listening");
        let _ = axum::serve(listener, health_app)
            .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
            .await;
    });

    let mut shard = Shard::new(ShardId::ONE, bot_token.to_owned(), intents);

    info!("starting Discord gateway connection");

    loop {
        let msg = poll_fn(|cx| Pin::new(&mut shard).poll_next(cx)).await;
        match msg {
            Some(Ok(Message::Text(text))) => bridge.dispatch(&text).await,
            Some(Ok(Message::Close(_))) => {
                info!("gateway connection closed");
                break;
            }
            Some(Err(source)) => {
                warn!(?source, "error receiving gateway message");
                continue;
            }
            None => break,
        }
    }
}

#[cfg(not(coverage))]
async fn run_webhook(
    js: NatsJetStreamClient,
    nats: async_nats::Client,
    public_key: ed25519_dalek::VerifyingKey,
    config: &DiscordConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("mode: webhook");

    let app = trogon_source_discord::router(js, nats, public_key, config);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "starting HTTP interactions endpoint");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
        .await?;

    Ok(())
}

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = DiscordConfig::from_env(&SystemEnv);

    acp_telemetry::init_logger(
        ServiceName::TrogonSourceDiscord,
        &config.subject_prefix,
        &SystemEnv,
        &SystemFs,
    );

    info!("Discord source starting");

    let nats = connect(&config.nats, DEFAULT_NATS_CONNECT_TIMEOUT).await?;
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));

    trogon_source_discord::provision(&js, &config)
        .await
        .map_err(|e| format!("stream provisioning failed: {e}"))?;

    match config.mode {
        SourceMode::Gateway {
            ref bot_token,
            intents,
        } => run_gateway(js, &config, bot_token, intents).await,
        SourceMode::Webhook { public_key } => run_webhook(js, nats, public_key, &config).await?,
    }

    info!("Discord source stopped");
    acp_telemetry::shutdown_otel();

    Ok(())
}

#[cfg(coverage)]
fn main() {}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
