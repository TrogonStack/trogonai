#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Discord channel bridge: the path from the gateway's raw Discord stream to
//! an ACP agent and back. See `docs/architecture/multi-channel-agent-routing.md`.
//!
//! The same two halves as the Telegram bridge: normalize (raw `MESSAGE_CREATE`
//! -> `InboundEvent`, identity + conversation via KV, prompt via `AgentPort`)
//! and render (agent session notifications -> Discord API calls). Everything
//! channel-neutral lives in `trogon-channel` and `channel-acp-port`; this
//! binary is allowed to know about Discord and nothing else.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod config;
mod constants;
mod outbound;
mod parse;
mod pipeline;
mod render;

use {
    acp_nats::AgentHandler,
    agent_client_protocol::schema::ProtocolVersion,
    agent_client_protocol::v1::schema::InitializeRequest,
    anyhow::Context as _,
    async_nats::jetstream::consumer::DeliverPolicy,
    channel_acp_port::{AcpBridge, AcpPort, BufferedRenderClient, SessionMethods},
    config::BridgeConfig,
    futures::StreamExt,
    outbound::DiscordOutbound,
    pipeline::Pipeline,
    std::sync::Arc,
    tracing::{error, info, warn},
    trogon_channel::store::PrincipalRecord,
    trogon_channel::{ChannelStore, PrincipalId, SafeToken},
    trogon_nats::jetstream::{ClaimResolver, NatsObjectStore},
    trogon_std::UuidV7Generator,
    trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
    trogon_std::signal::shutdown_signal,
    trogon_telemetry::ServiceName,
    twilight_model::id::Id,
    twilight_model::id::marker::UserMarker,
};

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> anyhow::Result<()> {
    let config = BridgeConfig::from_env(&SystemEnv)?;
    trogon_telemetry::init_logger(ServiceName::ChannelBridgeDiscord, [], &SystemEnv, &SystemFs);

    info!("Discord channel bridge starting");

    // Who this bot is decides what counts as being spoken to (a mention) and
    // which messages are its own. Asking Discord at boot, rather than taking
    // it as configuration, means the two can never disagree with the token.
    let http = twilight_http::Client::new(config.bot_token.as_str().to_string());
    let bot = http
        .current_user()
        .await
        .context("could not identify the bot; is DISCORD_BOT_TOKEN valid?")?
        .model()
        .await
        .context("could not read the bot's own user")?
        .id;
    info!(bot = %bot, "Identified Discord bot");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = acp_nats::nats::connect(config.acp.nats(), nats_connect_timeout).await?;
    let js = async_nats::jetstream::new(nats_client.clone());

    let store = ChannelStore::ensure(&js, &config.channel_prefix).await?;
    seed_principals(&store, &config).await?;

    let stream = js.get_stream(&config.inbound_stream).await.map_err(|e| {
        anyhow::anyhow!(
            "inbound stream '{}' not found; the trogon-gateway Discord source must provision it: {e}",
            config.inbound_stream
        )
    })?;
    // Same ownership as the stream: the gateway creates this bucket at startup.
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
                // The Discord stream carries every gateway event (presence,
                // reactions, member updates, ...); this bridge only reads new
                // messages, so the server filters rather than the bridge
                // acking and dropping the rest one by one.
                filter_subject: config.inbound_subject.clone(),
                deliver_policy: DeliverPolicy::New,
                ack_wait: std::time::Duration::from_secs(600),
                max_deliver: 5,
                ..Default::default()
            },
        )
        .await
        .context("failed to create inbound consumer")?;
    let messages = consumer.messages().await.context("failed to open inbound messages")?;

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run(
            nats_client,
            store,
            claims,
            messages,
            DiscordOutbound::new(http),
            bot,
            config,
            shutdown_signal(),
        ))
        .await;

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }
    result
}

/// Seeded users are linked by their user endpoint, which is also the endpoint
/// of their direct messages with the bot (see `parse::DiscordInbound`).
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

#[allow(clippy::too_many_arguments)]
async fn run<M, S>(
    nats_client: async_nats::Client,
    store: ChannelStore,
    claims: ClaimResolver<NatsObjectStore>,
    mut messages: M,
    discord: DiscordOutbound,
    bot: Id<UserMarker>,
    config: BridgeConfig,
    shutdown: S,
) -> anyhow::Result<()>
where
    M: futures::Stream<
            Item = Result<async_nats::jetstream::Message, async_nats::jetstream::consumer::pull::MessagesError>,
        > + Unpin,
    S: std::future::Future<Output = ()>,
{
    let meter = trogon_telemetry::meter(ServiceName::ChannelBridgeDiscord.as_str());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats_client.clone()));
    let bridge: Arc<AcpBridge> = Arc::new(acp_nats::Bridge::new(
        nats_client.clone(),
        js_client,
        trogon_std::time::SystemClock,
        &meter,
        config.acp.clone(),
    ));
    let renderer = Arc::new(BufferedRenderClient::new());

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
        "ACP agent initialized; consuming inbound messages"
    );

    let port = AcpPort::new(bridge.clone(), config.agent_cwd.clone(), session_methods);
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &discord,
        claims: &claims,
        account: &config.account,
        bot,
        agent_id: &config.agent_id,
        triggers: &config.command_triggers,
        ids: &UuidV7Generator,
    };

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
                            error!(error = ?e, "Failed to process message; leaving unacked for redelivery");
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
