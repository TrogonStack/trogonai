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
mod parse;
mod render;

use acp_port::{AcpBridge, AcpPort};
use agent_client_protocol::{Agent, Client, InitializeRequest, ProtocolVersion};
use anyhow::Context as _;
use config::BridgeConfig;
use futures::StreamExt;
use render::{TEXT_CHUNK_LIMIT, TelegramRenderClient, chunk_text};
use std::rc::Rc;
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};
use tracing::{error, info, warn};
use trogon_chat::store::PrincipalRecord;
use trogon_chat::{AgentId, AgentPort, ChatStore, ConversationRecord, Endpoint, PrincipalId};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_std::signal::shutdown_signal;
use trogon_telemetry::ServiceName;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

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
                        if let Err(e) = handle_message(&msg, &store, &port, &renderer, &bot, &config).await {
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

async fn ack(msg: &async_nats::jetstream::Message) -> anyhow::Result<()> {
    msg.ack().await.map_err(|e| anyhow::anyhow!("ack failed: {e}"))
}

async fn handle_message(
    msg: &async_nats::jetstream::Message,
    store: &ChatStore,
    port: &AcpPort,
    renderer: &Rc<TelegramRenderClient>,
    bot: &Bot,
    config: &BridgeConfig,
) -> anyhow::Result<()> {
    let update = match serde_json::from_slice::<teloxide::types::Update>(&msg.payload) {
        Ok(update) => update,
        Err(e) => {
            warn!(error = %e, body_len = msg.payload.len(), "Unparseable Telegram update; dropping");
            return ack(msg).await;
        }
    };

    let Some(event) = parse::inbound_event(&update, &config.bot_account) else {
        return ack(msg).await;
    };

    let Some(principal) = store.principal_for(&event.endpoint).await? else {
        info!(endpoint = %event.endpoint, "Unknown endpoint; ignoring (no principal linked)");
        return ack(msg).await;
    };

    let chat_id = ChatId(
        event
            .endpoint
            .peer()
            .parse::<i64>()
            .context("telegram peer is not an i64 chat id")?,
    );

    let now = now_unix();
    let (conversation_id, mut record) = match store.conversation_for(&event.endpoint).await? {
        Some(found) => found,
        None => {
            // Routing policy, v1: every new conversation binds to the single
            // configured agent. Sticky from here on.
            let record = ConversationRecord {
                principal: principal.clone(),
                agent_id: AgentId::new(&config.agent_id),
                current_session: None,
                created_at: now,
                last_activity_at: now,
            };
            let id = store.create_conversation(&event.endpoint, &record).await?;
            info!(conversation = %id, endpoint = %event.endpoint, agent = %record.agent_id, "Created conversation");
            (id, record)
        }
    };

    let mut active_session = match record.current_session.clone() {
        Some(session) => session,
        None => {
            let session = port.create_session(&record).await?;
            record.current_session = Some(session.clone());
            store.update_conversation(&conversation_id, &record).await?;
            session
        }
    };

    let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;

    let outcome = match port.prompt(&active_session, &event).await {
        Ok(outcome) => outcome,
        Err(first_error) => {
            // Sessions are ephemeral and belong to the agent: repair the
            // session in place, never re-run routing policy.
            warn!(error = %first_error, session = %active_session, "Prompt failed; retrying with a fresh session");
            let fresh = port.create_session(&record).await?;
            record.current_session = Some(fresh.clone());
            store.update_conversation(&conversation_id, &record).await?;
            active_session = fresh;
            port.prompt(&active_session, &event).await?
        }
    };

    record.last_activity_at = now_unix();
    store.update_conversation(&conversation_id, &record).await?;

    match renderer.take_buffer(active_session.as_str()) {
        Some(text) => {
            for chunk in chunk_text(&text, TEXT_CHUNK_LIMIT) {
                bot.send_message(chat_id, chunk)
                    .await
                    .context("telegram send failed")?;
            }
        }
        None => warn!(outcome = ?outcome, session = %active_session, "Agent turn produced no text"),
    }

    ack(msg).await
}
