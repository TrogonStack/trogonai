#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;

use crate::outbound::Outbound;
use crate::parse;
use crate::render::{TEXT_CHUNK_LIMIT, TelegramRenderClient, chunk_text};
use anyhow::Context as _;
use tracing::{info, warn};
use trogon_channel::{
    AgentId, AgentPort, AgentPortError as _, ChannelStore, Command, CommandTriggers, ConversationId,
    ConversationRecord, InboundEvent, ReleaseReason,
};
use trogon_std::NowV7;

/// What the bridge says back when a command has nothing else to do. A reset
/// with no follow-up prompt produces no agent output, so without this the user
/// gets silence.
const NEW_SESSION_ACKNOWLEDGEMENT: &str = "Started a new session.";

pub struct Pipeline<'a, P, O, G> {
    pub store: &'a ChannelStore,
    pub port: &'a P,
    pub renderer: &'a TelegramRenderClient,
    pub outbound: &'a O,
    pub bot_account: &'a str,
    pub agent_id: &'a str,
    pub triggers: &'a CommandTriggers,
    pub ids: &'a G,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

async fn ack(msg: &async_nats::jetstream::Message) -> anyhow::Result<()> {
    msg.ack().await.map_err(|e| anyhow::anyhow!("ack failed: {e}"))
}

impl<P: AgentPort, O: Outbound, G: NowV7> Pipeline<'_, P, O, G> {
    /// Whether the individual who sent this message is a known principal. The
    /// conversation gate authorizes the chat, which in a group is everyone in
    /// it; destructive commands ask the narrower question.
    async fn sender_is_authorized(&self, event: &InboundEvent) -> anyhow::Result<bool> {
        let Some(endpoint) = parse::sender_endpoint(self.bot_account, &event.sender) else {
            return Ok(false);
        };
        Ok(self.store.principal_for(&endpoint).await?.is_some())
    }

    /// Drop the conversation's pointer to its session, then tell the agent it
    /// can let go. In that order: a crash in between leaves an orphaned agent
    /// session, which costs the agent some memory, where the reverse order
    /// resurrects a session the user just asked to be rid of. Redelivery is
    /// safe for the same reason, since the pointer is already gone.
    async fn release_current_session(
        &self,
        conversation_id: &ConversationId,
        record: &mut ConversationRecord,
    ) -> anyhow::Result<()> {
        let Some(session) = record.current_session.take() else {
            return Ok(());
        };
        record.last_activity_at = now_unix();
        self.store.update_conversation(conversation_id, record).await?;
        self.renderer.discard(session.as_str());

        let release = self.port.release_session(&session, ReleaseReason::NewSession).await;
        info!(
            conversation = %conversation_id,
            session = %session,
            cancelled = ?release.cancelled,
            closed = ?release.closed,
            "Released session"
        );
        Ok(())
    }

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

        let Some(event) = parse::inbound_event(&update, self.bot_account, self.triggers) else {
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
                let id = self
                    .store
                    .create_conversation(&event.endpoint, &record, self.ids)
                    .await?;
                info!(conversation = %id, endpoint = %event.endpoint, agent = %record.agent_id, "Created conversation");
                (id, record)
            }
        };

        if event.command == Some(Command::NewSession) {
            if self.sender_is_authorized(&event).await? {
                self.release_current_session(&conversation_id, &mut record).await?;
                if event.text.is_none() {
                    self.outbound
                        .send_text(chat_id, NEW_SESSION_ACKNOWLEDGEMENT.to_string())
                        .await
                        .context("telegram send failed")?;
                    return ack(msg).await;
                }
            } else {
                // The command is refused, not the message. Whatever followed
                // the trigger is still the user talking to the agent, and the
                // trigger itself is never forwarded.
                warn!(
                    conversation = %conversation_id,
                    sender = %event.sender.platform_user_id,
                    "Sender is not a linked principal; ignoring new-session command"
                );
            }
        }

        if event.text.is_none() {
            return ack(msg).await;
        }

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
            // Only a session the agent no longer has is repaired here, and
            // repaired in place: sessions are ephemeral and belong to the
            // agent, so routing policy never re-runs. Every other failure is
            // left to redelivery, because rotating on a timeout or a transport
            // blip would throw away a conversation that was merely unreachable.
            Err(first_error) if first_error.is_session_lost() => {
                warn!(error = %first_error, session = %active_session, "Agent no longer has the session; retrying with a fresh one");
                let fresh = self
                    .port
                    .create_session(&record)
                    .await
                    .map_err(|e| anyhow::anyhow!("create_session failed: {e}"))?;
                record.current_session = Some(fresh.clone());
                self.store.update_conversation(&conversation_id, &record).await?;
                self.renderer.discard(active_session.as_str());
                active_session = fresh;
                self.port
                    .prompt(&active_session, &event)
                    .await
                    .map_err(|e| anyhow::anyhow!("prompt retry failed: {e}"))?
            }
            Err(error) => return Err(anyhow::anyhow!("prompt failed: {error}")),
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
