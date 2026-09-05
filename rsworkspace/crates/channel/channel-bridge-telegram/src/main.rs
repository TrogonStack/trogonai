//! Telegram channel bridge: the path from the gateway's raw Telegram stream to
//! an ACP agent. See `docs/architecture/multi-channel-agent-routing.md`.
//!
//! One worker, two halves: normalize (raw Update -> `InboundEvent`,
//! identity + conversation via KV, prompt via `AgentPort`) and render (agent
//! session notifications -> Telegram API calls). Everything channel-neutral
//! lives in `trogon-channel`; this binary is allowed to know about Telegram and
//! nothing else.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod acp_port;
mod config;
mod constants;
mod outbound;
mod parse;
mod pipeline;
mod render;

use {
    acp_nats::AgentHandler,
    acp_port::{AcpBridge, AcpPort, SessionMethods},
    agent_client_protocol::schema::ProtocolVersion,
    agent_client_protocol::schema::v1::InitializeRequest,
    anyhow::Context as _,
    async_nats::jetstream::consumer::DeliverPolicy,
    config::BridgeConfig,
    futures::StreamExt,
    outbound::TelegramOutbound,
    pipeline::Pipeline,
    render::TelegramRenderClient,
    std::sync::Arc,
    teloxide::Bot,
    tracing::{error, info, warn},
    trogon_channel::store::PrincipalRecord,
    trogon_channel::{ChannelStore, PrincipalId, SafeToken},
    trogon_nats::jetstream::{ClaimResolver, NatsObjectStore},
    trogon_std::UuidV7Generator,
    trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
    trogon_std::signal::shutdown_signal,
    trogon_telemetry::ServiceName,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = BridgeConfig::from_env(&SystemEnv)?;
    trogon_telemetry::init_logger(ServiceName::ChannelBridgeTelegram, [], &SystemEnv, &SystemFs);

    info!("Telegram channel bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = acp_nats::nats::connect(config.acp.nats(), nats_connect_timeout).await?;
    let js = async_nats::jetstream::new(nats_client.clone());

    let store = ChannelStore::ensure(&js, &config.channel_prefix).await?;
    seed_principals(&store, &config).await?;

    let stream = js.get_stream(&config.inbound_stream).await.map_err(|e| {
        anyhow::anyhow!(
            "inbound stream '{}' not found; the trogon-gateway Telegram source must provision it: {e}",
            config.inbound_stream
        )
    })?;
    // Same ownership as the stream: the gateway creates this bucket at startup
    // and sizes its retention against the longest-retained stream it serves.
    // Refusing to start without it turns a missing gateway into a boot failure,
    // rather than into an oversized update that arrives one day and cannot be
    // redeemed.
    let claims = ClaimResolver::new(
        NatsObjectStore::bind_claim_bucket(&js, config.claim_bucket.clone())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "claim bucket '{}' not found; the trogon-gateway must provision it: {e}",
                    config.claim_bucket
                )
            })?,
    );
    let consumer_name = format!("{}-{}", constants::INBOUND_DURABLE, config.channel_prefix);
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                // Honoured only when the durable does not exist yet, so a
                // first run answers what arrives from now on instead of
                // replaying everything the stream still retains. Restarts
                // resume from the durable's own ack floor.
                deliver_policy: DeliverPolicy::New,
                // Generous ack window: a prompt turn can legitimately run for
                // minutes before the turn ends and we ack.
                ack_wait: std::time::Duration::from_secs(600),
                max_deliver: 5,
                ..Default::default()
            },
        )
        .await
        .context("failed to create inbound consumer")?;
    let messages = consumer.messages().await.context("failed to open inbound messages")?;

    let bot = Bot::new(config.bot_token.as_str());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run(nats_client, store, claims, messages, bot, config))
        .await;

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }
    result
}

async fn seed_principals(store: &ChannelStore, config: &BridgeConfig) -> anyhow::Result<()> {
    for user in &config.seed_users {
        let principal = PrincipalId::new(format!("{}-{user}", constants::CHANNEL))?;
        let endpoint = config.account.endpoint_for(&SafeToken::from(*user));
        store
            .link_endpoint(&principal, &PrincipalRecord { display_name: None }, &endpoint)
            .await?;
        info!(principal = %principal, endpoint = %endpoint, "Seeded principal");
    }
    Ok(())
}

async fn run(
    nats_client: async_nats::Client,
    store: ChannelStore,
    claims: ClaimResolver<NatsObjectStore>,
    mut messages: async_nats::jetstream::consumer::pull::Stream,
    bot: Bot,
    config: BridgeConfig,
) -> anyhow::Result<()> {
    let meter = trogon_telemetry::meter("channel-bridge-telegram");
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats_client.clone()));
    let bridge: Arc<AcpBridge> = Arc::new(acp_nats::Bridge::new(
        nats_client.clone(),
        js_client,
        trogon_std::time::SystemClock,
        &meter,
        config.acp.clone(),
    ));
    let renderer = Arc::new(TelegramRenderClient::new());

    let mut client_task = tokio::task::spawn_local(acp_nats::client::run(
        nats_client.clone(),
        renderer.clone(),
        bridge.clone(),
    ));
    let initialized = bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e}"))?;
    let session_methods = SessionMethods::advertised(&initialized);
    info!(
        protocol = %initialized.protocol_version,
        session_methods = %session_methods,
        "ACP agent initialized; consuming inbound updates"
    );

    let port = AcpPort::new(bridge.clone(), config.agent_cwd.clone(), session_methods);
    let telegram = TelegramOutbound::new(bot);
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &telegram,
        claims: &claims,
        account: &config.account,
        agent_id: &config.agent_id,
        triggers: &config.command_triggers,
        ids: &UuidV7Generator,
    };

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("Shutting down");
                break;
            }
            result = &mut client_task => {
                error!(?result, "ACP client task ended; agent responses can no longer be rendered");
                break;
            }
            next = messages.next() => {
                let Some(next) = next else {
                    warn!("Inbound consumer stream ended");
                    break;
                };
                match next {
                    Ok(msg) => {
                        if let Err(e) = pipeline.handle_message(&msg).await {
                            // Left unacked on purpose: JetStream redelivers
                            // (max_deliver bounds the retries).
                            error!(error = ?e, "Failed to process update; leaving unacked for redelivery");
                        }
                    }
                    Err(e) => warn!(error = %e, "Error receiving from inbound consumer"),
                }
            }
        }
    }

    client_task.abort();
    Ok(())
}
