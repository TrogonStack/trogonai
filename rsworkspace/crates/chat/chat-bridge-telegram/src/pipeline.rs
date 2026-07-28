#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;

use crate::outbound::Outbound;
use crate::parse;
use crate::render::{TEXT_CHUNK_LIMIT, TelegramRenderClient, chunk_text};
use anyhow::Context as _;
use tracing::{info, warn};
use trogon_chat::{AgentId, AgentPort, ChatStore, ConversationRecord};

pub struct Pipeline<'a, P, O> {
    pub store: &'a ChatStore,
    pub port: &'a P,
    pub renderer: &'a TelegramRenderClient,
    pub outbound: &'a O,
    pub bot_account: &'a str,
    pub agent_id: &'a str,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

async fn ack(msg: &async_nats::jetstream::Message) -> anyhow::Result<()> {
    msg.ack().await.map_err(|e| anyhow::anyhow!("ack failed: {e}"))
}

impl<P: AgentPort, O: Outbound> Pipeline<'_, P, O> {
    /// Process one raw gateway message end to end. Unrecoverable messages
    /// (unparseable, unauthorized, kinds v1 does not carry) are acked and
    /// dropped; processing errors return `Err` with the message unacked so
    /// JetStream redelivers.
    pub async fn handle_message(&self, msg: &async_nats::jetstream::Message) -> anyhow::Result<()> {
        let update = match serde_json::from_slice::<teloxide::types::Update>(&msg.payload) {
            Ok(update) => update,
            Err(e) => {
                warn!(error = %e, body_len = msg.payload.len(), "Unparseable Telegram update; dropping");
                return ack(msg).await;
            }
        };

        let Some(event) = parse::inbound_event(&update, self.bot_account) else {
            return ack(msg).await;
        };

        let Some(principal) = self.store.principal_for(&event.endpoint).await? else {
            info!(endpoint = %event.endpoint, "Unknown endpoint; ignoring (no principal linked)");
            return ack(msg).await;
        };

        let chat_id = event
            .endpoint
            .peer()
            .parse::<i64>()
            .context("telegram peer is not an i64 chat id")?;

        let now = now_unix();
        let (conversation_id, mut record) = match self.store.conversation_for(&event.endpoint).await? {
            Some(found) => found,
            None => {
                // Routing policy, v1: every new conversation binds to the
                // single configured agent. Sticky from here on.
                let record = ConversationRecord {
                    principal: principal.clone(),
                    agent_id: AgentId::new(self.agent_id),
                    current_session: None,
                    created_at: now,
                    last_activity_at: now,
                };
                let id = self.store.create_conversation(&event.endpoint, &record).await?;
                info!(conversation = %id, endpoint = %event.endpoint, agent = %record.agent_id, "Created conversation");
                (id, record)
            }
        };

        let mut active_session = match record.current_session.clone() {
            Some(session) => session,
            None => {
                let session = self
                    .port
                    .create_session(&record)
                    .await
                    .map_err(|e| anyhow::anyhow!("create_session failed: {e}"))?;
                record.current_session = Some(session.clone());
                self.store.update_conversation(&conversation_id, &record).await?;
                session
            }
        };

        let _ = self.outbound.typing(chat_id).await;

        let outcome = match self.port.prompt(&active_session, &event).await {
            Ok(outcome) => outcome,
            Err(first_error) => {
                // Sessions are ephemeral and belong to the agent: repair the
                // session in place, never re-run routing policy.
                warn!(error = %first_error, session = %active_session, "Prompt failed; retrying with a fresh session");
                let fresh = self
                    .port
                    .create_session(&record)
                    .await
                    .map_err(|e| anyhow::anyhow!("create_session failed: {e}"))?;
                record.current_session = Some(fresh.clone());
                self.store.update_conversation(&conversation_id, &record).await?;
                active_session = fresh;
                self.port
                    .prompt(&active_session, &event)
                    .await
                    .map_err(|e| anyhow::anyhow!("prompt retry failed: {e}"))?
            }
        };

        record.last_activity_at = now_unix();
        self.store.update_conversation(&conversation_id, &record).await?;

        match self.renderer.take_buffer(active_session.as_str()) {
            Some(text) => {
                for chunk in chunk_text(&text, TEXT_CHUNK_LIMIT) {
                    self.outbound
                        .send_text(chat_id, chunk)
                        .await
                        .context("telegram send failed")?;
                }
            }
            None => warn!(outcome = ?outcome, session = %active_session, "Agent turn produced no text"),
        }

        ack(msg).await
    }
}
