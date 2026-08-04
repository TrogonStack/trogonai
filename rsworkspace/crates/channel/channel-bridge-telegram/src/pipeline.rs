#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;

use crate::constants::{NEW_SESSION_ACKNOWLEDGEMENT, TEXT_CHUNK_LIMIT};
use crate::outbound::Outbound;
use crate::parse;
use crate::render::{TelegramRenderClient, chunk_text};
use anyhow::Context as _;
use tracing::{info, warn};
use trogon_channel::{
    AgentId, AgentPort, AgentPortError as _, ChannelStore, Command, CommandTriggers, ConversationId,
    ConversationRecord, InboundEvent, ReleaseReason,
};
use trogon_nats::jetstream::{ClaimResolver, ObjectStoreGet};
use trogon_std::NowV7;

pub struct Pipeline<'a, P, O, G, S> {
    pub store: &'a ChannelStore,
    pub port: &'a P,
    pub renderer: &'a TelegramRenderClient,
    pub outbound: &'a O,
    pub claims: &'a ClaimResolver<S>,
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

impl<P: AgentPort, O: Outbound, G: NowV7, S: ObjectStoreGet> Pipeline<'_, P, O, G, S> {
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
    /// (unparseable, unauthorized, kinds the bridge does not carry) are acked and
    /// dropped; processing errors return `Err` with the message unacked so
    /// JetStream redelivers.
    pub async fn handle_message(&self, msg: &async_nats::jetstream::Message) -> anyhow::Result<()> {
        // An update over the NATS max payload reaches the stream as an empty
        // body plus claim headers, so the parse below has to run on the redeemed
        // bytes. A failure here returns Err rather than acking: the update is
        // real and recoverable, and dropping it would lose it permanently while
        // blaming the parser.
        let body = self
            .claims
            .resolve(msg.headers.as_ref(), msg.payload.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to redeem claim-checked update: {e}"))?;

        let update = match serde_json::from_slice::<teloxide::types::Update>(&body) {
            Ok(update) => update,
            Err(e) => {
                warn!(error = %e, body_len = body.len(), "Unparseable Telegram update; dropping");
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
                // Routing policy: every new conversation binds to the single
                // configured agent. Sticky from here on.
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
            //
            // Whether the session is really gone is a guess (see
            // `AgentPortError::is_session_lost`), so nothing is committed until
            // the fresh session has answered. A wrong guess then costs one unused
            // session instead of the conversation's history, which is the whole
            // reason the pointer is not moved first.
            Err(first_error) if first_error.is_session_lost() => {
                warn!(error = %first_error, session = %active_session, "Agent may no longer have the session; trying a fresh one");
                let fresh = self
                    .port
                    .create_session(&record)
                    .await
                    .map_err(|e| anyhow::anyhow!("create_session failed: {e}"))?;

                match self.port.prompt(&fresh, &event).await {
                    Ok(outcome) => {
                        record.current_session = Some(fresh.clone());
                        self.store.update_conversation(&conversation_id, &record).await?;
                        self.renderer.discard(active_session.as_str());
                        active_session = fresh;
                        outcome
                    }
                    // A fresh session failed the same way, so the session was
                    // never the problem: the prompt itself is being rejected.
                    // Hand back the session nobody used and leave the
                    // conversation where it was, so redelivery retries the
                    // prompt rather than compounding the rotation.
                    Err(retry_error) => {
                        let release = self.port.release_session(&fresh, ReleaseReason::RepairFailed).await;
                        self.renderer.discard(fresh.as_str());
                        info!(
                            conversation = %conversation_id,
                            session = %fresh,
                            cancelled = ?release.cancelled,
                            closed = ?release.closed,
                            "Released the session opened to repair a suspected loss"
                        );
                        return Err(anyhow::anyhow!(
                            "prompt failed on session {active_session} and again on a fresh session, so the session was not the cause: {first_error} (retry: {retry_error})"
                        ));
                    }
                }
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
