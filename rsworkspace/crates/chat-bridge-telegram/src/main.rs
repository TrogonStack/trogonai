//! Telegram chat bridge: the v1 direct path from the gateway's raw Telegram
//! stream to an ACP agent. See `docs/architecture/multi-channel-agent-routing.md`.
//!
//! One worker, two halves: normalize (raw Update -> `InboundChatEvent`,
//! identity + conversation via KV, prompt via `AgentPort`) and render (agent
//! session notifications -> Telegram API calls). Everything channel-neutral
//! lives in `trogon-chat`; this binary is allowed to know about Telegram and
//! nothing else.

mod acp_port;
mod config;
mod outbound;
mod parse;
mod pipeline;
mod render;

use acp_port::{AcpBridge, AcpPort};
use agent_client_protocol::{Agent, Client, InitializeRequest, ProtocolVersion};
use anyhow::Context as _;
use config::BridgeConfig;
use futures::StreamExt;
use outbound::TelegramOutbound;
use pipeline::Pipeline;
use render::TelegramRenderClient;
use std::rc::Rc;
use teloxide::Bot;
use tracing::{error, info, warn};
use trogon_chat::store::PrincipalRecord;
use trogon_chat::{ChatStore, Endpoint, PrincipalId};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_std::signal::shutdown_signal;
use trogon_telemetry::ServiceName;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = BridgeConfig::from_env(&SystemEnv)?;
    trogon_telemetry::init_logger(ServiceName::ChatBridgeTelegram, [], &SystemEnv, &SystemFs);

    info!("Telegram chat bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = acp_nats::nats::connect(config.acp.nats(), nats_connect_timeout).await?;
    let js = async_nats::jetstream::new(nats_client.clone());

    let store = ChatStore::ensure(&js, &config.chat_prefix).await?;
    seed_principals(&store, &config).await?;

    let stream = js.get_stream(&config.inbound_stream).await.map_err(|e| {
        anyhow::anyhow!(
            "inbound stream '{}' not found; the trogon-gateway Telegram source must provision it: {e}",
            config.inbound_stream
        )
    })?;
    let consumer_name = format!("chat-bridge-telegram-{}", config.chat_prefix);
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
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

    let bot = Bot::new(config.bot_token.clone());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run(nats_client, store, messages, bot, config))
        .await;

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }
    result
}

async fn seed_principals(store: &ChatStore, config: &BridgeConfig) -> anyhow::Result<()> {
    for user in &config.seed_users {
        let principal = PrincipalId::new(format!("telegram-{user}"))?;
        let endpoint = Endpoint::new("telegram", &config.bot_account, user.to_string())?;
        store
            .link_endpoint(&principal, &PrincipalRecord { display_name: None }, &endpoint)
            .await?;
        info!(principal = %principal, endpoint = %endpoint, "Seeded principal");
    }
    Ok(())
}

async fn run(
    nats_client: async_nats::Client,
    store: ChatStore,
    mut messages: async_nats::jetstream::consumer::pull::Stream,
    bot: Bot,
    config: BridgeConfig,
) -> anyhow::Result<()> {
    let meter = trogon_telemetry::meter("chat-bridge-telegram");
    let (notification_tx, mut notification_rx) = tokio::sync::mpsc::channel(64);
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats_client.clone()));
    let bridge: Rc<AcpBridge> = Rc::new(acp_nats::Bridge::new(
        nats_client.clone(),
        js_client,
        trogon_std::time::SystemClock,
        &meter,
        config.acp.clone(),
        notification_tx,
    ));
    let renderer = Rc::new(TelegramRenderClient::new());

    let client_task = tokio::task::spawn_local(acp_nats::client::run(
        nats_client.clone(),
        renderer.clone(),
        bridge.clone(),
    ));
    let renderer_for_rx = renderer.clone();
    let notification_task = tokio::task::spawn_local(async move {
        while let Some(notification) = notification_rx.recv().await {
            if renderer_for_rx.session_notification(notification).await.is_err() {
                break;
            }
        }
    });

    bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e}"))?;
    info!("ACP agent initialized; consuming inbound updates");

    let port = AcpPort::new(bridge.clone(), config.agent_cwd.clone());
    let telegram = TelegramOutbound::new(bot);
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &telegram,
        bot_account: &config.bot_account,
        agent_id: &config.agent_id,
    };

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("Shutting down");
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
    notification_task.abort();
    Ok(())
}
