use std::sync::Arc;

use acp_nats::prompt_event::{PromptEvent, PromptPayload};
use acp_nats::nats::agent as subjects;
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use tracing::{error, info, warn};

use trogon_agent::agent_loop::{AgentLoop, Message};
use trogon_agent::tools::all_tool_defs;

use crate::session_store::SessionStore;

/// Subscribes to `{prefix}.*.agent.prompt` via NATS Core, runs the agentic loop
/// for each incoming prompt, and publishes `PromptEvent` messages back to the Bridge.
pub struct Runner {
    nats: async_nats::Client,
    store: SessionStore,
    agent: Arc<AgentLoop>,
    prefix: String,
}

impl Runner {
    pub async fn new(
        nats: async_nats::Client,
        js: &jetstream::Context,
        agent: AgentLoop,
        prefix: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let store = SessionStore::open(js).await?;
        Ok(Self {
            nats,
            store,
            agent: Arc::new(agent),
            prefix: prefix.into(),
        })
    }

    /// Run the prompt subscriber loop — returns when the NATS connection closes.
    pub async fn run(self) {
        let wildcard = subjects::prompt_wildcard(&self.prefix);
        let mut sub = match self.nats.subscribe(wildcard.clone()).await {
            Ok(s) => s,
            Err(e) => {
                error!(subject = %wildcard, error = %e, "runner: failed to subscribe");
                return;
            }
        };

        info!(subject = %wildcard, "runner: listening for prompts");

        while let Some(msg) = sub.next().await {
            let payload: PromptPayload = match serde_json::from_slice(&msg.payload) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "runner: bad prompt payload — skipping");
                    continue;
                }
            };

            let events_subject = subjects::prompt_events(
                &self.prefix,
                &payload.session_id,
                &payload.req_id,
            );

            self.handle_prompt(payload, events_subject).await;
        }

        info!("runner: subscription stream ended");
    }

    async fn handle_prompt(&self, payload: PromptPayload, events_subject: String) {
        // Load session history from KV
        let mut state = match self.store.load(&payload.session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id = %payload.session_id, error = %e, "runner: failed to load session");
                self.publish_error(&events_subject, format!("session load failed: {e}")).await;
                return;
            }
        };

        // Append the user turn
        state.messages.push(Message::user_text(&payload.user_message));

        // Run the agentic loop
        let tools = all_tool_defs();
        let result = self
            .agent
            .run_chat(state.messages.clone(), &tools, None)
            .await;

        match result {
            Ok((text, updated_messages)) => {
                // Save updated history
                state.messages = updated_messages;
                if let Err(e) = self.store.save(&payload.session_id, &state).await {
                    warn!(session_id = %payload.session_id, error = %e, "runner: failed to save session");
                }

                // Publish the response text
                self.publish_event(
                    &events_subject,
                    &PromptEvent::TextDelta { text },
                )
                .await;

                // Signal completion
                self.publish_event(
                    &events_subject,
                    &PromptEvent::Done { stop_reason: "end_turn".to_string() },
                )
                .await;
            }
            Err(e) => {
                self.publish_error(&events_subject, e.to_string()).await;
            }
        }
    }

    async fn publish_event(&self, subject: &str, event: &PromptEvent) {
        match serde_json::to_vec(event) {
            Ok(bytes) => {
                if let Err(e) = self.nats.publish(subject.to_string(), Bytes::from(bytes)).await {
                    warn!(subject, error = %e, "runner: failed to publish event");
                }
            }
            Err(e) => {
                warn!(error = %e, "runner: failed to serialize event");
            }
        }
    }

    async fn publish_error(&self, subject: &str, message: String) {
        self.publish_event(subject, &PromptEvent::Error { message }).await;
    }
}
