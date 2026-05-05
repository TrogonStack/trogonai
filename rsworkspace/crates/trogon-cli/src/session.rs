use agent_client_protocol::{
    ContentBlock, NewSessionRequest, PromptRequest, SessionNotification, SessionUpdate, TextContent,
};
use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(15);

pub struct TrogonSession {
    nats: Client,
    pub session_id: String,
    prefix: String,
}

impl TrogonSession {
    /// Create a new ACP session via NATS. `cwd` is the working directory for the session.
    pub async fn new(nats: Client, prefix: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let subject = format!("{prefix}.agent.session.new");
        let req = NewSessionRequest::new(cwd);
        let payload = serde_json::to_vec(&req)?;

        let reply = tokio::time::timeout(
            SESSION_NEW_TIMEOUT,
            nats.request(subject, payload.into()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for session creation (is trogon-acp-runner running?)"))?
        .map_err(|e| anyhow::anyhow!("NATS error creating session: {e}"))?;

        let resp: Value = serde_json::from_slice(&reply.payload)
            .map_err(|e| anyhow::anyhow!("invalid session response: {e}"))?;

        let session_id = resp["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session response missing sessionId: {resp}"))?
            .to_string();

        Ok(Self {
            nats,
            session_id,
            prefix: prefix.to_string(),
        })
    }

    /// Send a prompt and stream `TextDelta` events via the returned channel.
    /// The channel closes when the turn is done (Done or Error).
    pub async fn prompt(&self, text: &str) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let notif_subject = format!(
            "{}.session.{}.client.session.update",
            self.prefix, self.session_id
        );
        let prompt_subject = format!(
            "{}.session.{}.agent.prompt",
            self.prefix, self.session_id
        );

        let mut notif_sub = self
            .nats
            .subscribe(notif_subject)
            .await
            .map_err(|e| anyhow::anyhow!("subscribe notifications: {e}"))?;

        let inbox = self.nats.new_inbox();
        let mut resp_sub = self
            .nats
            .subscribe(inbox.clone())
            .await
            .map_err(|e| anyhow::anyhow!("subscribe inbox: {e}"))?;

        let req = PromptRequest::new(
            self.session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(text))],
        );
        let payload = serde_json::to_vec(&req)?;

        self.nats
            .publish_with_reply(prompt_subject, inbox, payload.into())
            .await
            .map_err(|e| anyhow::anyhow!("publish prompt: {e}"))?;

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    msg = resp_sub.next() => {
                        let Some(msg) = msg else { break };
                        if let Ok(v) = serde_json::from_slice::<Value>(&msg.payload) {
                            let stop = v.get("stopReason")
                                .and_then(|s| s.as_str())
                                .unwrap_or("end_turn")
                                .to_string();
                            let _ = tx.send(StreamEvent::Done(stop)).await;
                        } else {
                            let _ = tx.send(StreamEvent::Done("end_turn".into())).await;
                        }
                        break;
                    }
                    msg = notif_sub.next() => {
                        let Some(msg) = msg else { break };
                        if let Ok(notif) = serde_json::from_slice::<SessionNotification>(&msg.payload) {
                            match notif.update {
                                SessionUpdate::AgentMessageChunk(chunk) => {
                                    if let ContentBlock::Text(t) = chunk.content {
                                        let _ = tx.send(StreamEvent::Text(t.text)).await;
                                    }
                                }
                                SessionUpdate::AgentThoughtChunk(_chunk) => {
                                    let _ = tx.send(StreamEvent::Thinking).await;
                                }
                                SessionUpdate::ToolCall(tc) => {
                                    let _ = tx.send(StreamEvent::ToolCall(tc.title)).await;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Fire-and-forget cancel for the active prompt turn.
    pub async fn cancel(&self) {
        let subject = format!(
            "{}.session.{}.agent.cancel",
            self.prefix, self.session_id
        );
        let _ = self.nats.publish(subject, Bytes::new()).await;
    }
}

/// Events produced during a prompt turn.
#[derive(Debug)]
pub enum StreamEvent {
    Text(String),
    Thinking,
    ToolCall(String),
    Done(String),
}
