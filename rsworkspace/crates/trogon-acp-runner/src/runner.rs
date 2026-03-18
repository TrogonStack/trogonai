use std::sync::Arc;

use acp_nats::nats::agent as subjects;
use acp_nats::prompt_event::{PromptEvent, PromptPayload};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use trogon_agent::agent_loop::{AgentEvent, AgentLoop};
use trogon_agent::tools::all_tool_defs;

use crate::session_store::SessionStore;

/// Subscribes to `{prefix}.*.agent.prompt` via NATS Core, runs the agentic loop
/// for each incoming prompt (with streaming events and cancel support), and publishes
/// `PromptEvent` messages back to the Bridge.
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
        // Subscribe to the cancel subject for this session so we can abort mid-run
        let cancel_subject =
            subjects::session_cancel(&self.prefix, &payload.session_id);
        let mut cancel_sub = match self.nats.subscribe(cancel_subject.clone()).await {
            Ok(s) => s,
            Err(e) => {
                warn!(subject = %cancel_subject, error = %e, "runner: could not subscribe to cancel");
                // Proceed without cancel support rather than aborting the prompt
                return self.handle_prompt_no_cancel(payload, events_subject).await;
            }
        };

        // Load session history from KV
        let mut state = match self.store.load(&payload.session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id = %payload.session_id, error = %e, "runner: failed to load session");
                self.publish_error(&events_subject, format!("session load failed: {e}")).await;
                return;
            }
        };

        // Capture the first prompt as the session title (before appending the user turn)
        if state.title.is_empty() {
            state.title = truncate_title(&payload.user_message);
        }

        // Append the user turn
        state.messages.push(trogon_agent::agent_loop::Message::user_text(&payload.user_message));

        // Channel for streaming agent events
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);

        let tools = all_tool_defs();
        // Use per-session model override when present
        let agent: Arc<AgentLoop> = if let Some(ref model) = state.model {
            let mut a = (*self.agent).clone();
            a.model = model.clone();
            Arc::new(a)
        } else {
            self.agent.clone()
        };
        let messages = state.messages.clone();

        // Spawn the agent loop so we can select! against cancel
        let agent_fut = tokio::task::spawn_local(async move {
            agent.run_chat_streaming(messages, &tools, None, event_tx).await
        });

        // Forward streaming events to NATS while watching for cancel
        let mut final_messages: Option<Vec<trogon_agent::agent_loop::Message>> = None;
        let mut cancelled = false;

        loop {
            tokio::select! {
                // Agent event available
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let prompt_event = match event {
                                AgentEvent::TextDelta { text } => {
                                    PromptEvent::TextDelta { text }
                                }
                                AgentEvent::ThinkingDelta { text } => {
                                    PromptEvent::ThinkingDelta { text }
                                }
                                AgentEvent::ToolCallStarted { id, name, input } => {
                                    PromptEvent::ToolCallStarted { id, name, input }
                                }
                                AgentEvent::ToolCallFinished { id, output } => {
                                    PromptEvent::ToolCallFinished { id, output }
                                }
                                AgentEvent::UsageSummary { input_tokens, output_tokens } => {
                                    PromptEvent::UsageUpdate { input_tokens, output_tokens }
                                }
                            };
                            self.publish_event(&events_subject, &prompt_event).await;
                        }
                        None => {
                            // Channel closed — agent loop is done; join the task
                            match agent_fut.await {
                                Ok(Ok(updated_messages)) => {
                                    final_messages = Some(updated_messages);
                                }
                                Ok(Err(trogon_agent::agent_loop::AgentError::MaxIterationsReached)) => {
                                    self.publish_event(
                                        &events_subject,
                                        &PromptEvent::Done { stop_reason: "max_turn_requests".to_string() },
                                    ).await;
                                }
                                Ok(Err(e)) => {
                                    self.publish_error(&events_subject, e.to_string()).await;
                                }
                                Err(e) => {
                                    self.publish_error(&events_subject, format!("agent task panicked: {e}")).await;
                                }
                            }
                            break;
                        }
                    }
                }

                // Cancel arrived
                _ = cancel_sub.next() => {
                    info!(session_id = %payload.session_id, "runner: cancel received");
                    cancelled = true;
                    agent_fut.abort();
                    break;
                }
            }
        }

        if cancelled {
            self.publish_event(
                &events_subject,
                &PromptEvent::Done { stop_reason: "cancelled".to_string() },
            )
            .await;
            return;
        }

        if let Some(updated_messages) = final_messages {
            state.messages = updated_messages;
            if let Err(e) = self.store.save(&payload.session_id, &state).await {
                warn!(session_id = %payload.session_id, error = %e, "runner: failed to save session");
            }
            self.publish_event(
                &events_subject,
                &PromptEvent::Done { stop_reason: "end_turn".to_string() },
            )
            .await;
        }
    }

    /// Fallback path when we cannot subscribe to the cancel subject.
    async fn handle_prompt_no_cancel(&self, payload: PromptPayload, events_subject: String) {
        let mut state = match self.store.load(&payload.session_id).await {
            Ok(s) => s,
            Err(e) => {
                error!(session_id = %payload.session_id, error = %e, "runner: failed to load session");
                self.publish_error(&events_subject, format!("session load failed: {e}")).await;
                return;
            }
        };

        if state.title.is_empty() {
            state.title = truncate_title(&payload.user_message);
        }

        state.messages.push(trogon_agent::agent_loop::Message::user_text(&payload.user_message));

        let tools = all_tool_defs();
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);
        let agent: Arc<AgentLoop> = if let Some(ref model) = state.model {
            let mut a = (*self.agent).clone();
            a.model = model.clone();
            Arc::new(a)
        } else {
            self.agent.clone()
        };
        let messages = state.messages.clone();

        tokio::task::spawn_local(async move {
            let _ = agent.run_chat_streaming(messages, &tools, None, event_tx).await;
        });

        while let Some(event) = event_rx.recv().await {
            let prompt_event = match event {
                AgentEvent::TextDelta { text } => PromptEvent::TextDelta { text },
                AgentEvent::ThinkingDelta { text } => PromptEvent::ThinkingDelta { text },
                AgentEvent::ToolCallStarted { id, name, input } => {
                    PromptEvent::ToolCallStarted { id, name, input }
                }
                AgentEvent::ToolCallFinished { id, output } => {
                    PromptEvent::ToolCallFinished { id, output }
                }
                AgentEvent::UsageSummary { input_tokens, output_tokens } => {
                    PromptEvent::UsageUpdate { input_tokens, output_tokens }
                }
            };
            self.publish_event(&events_subject, &prompt_event).await;
        }

        self.publish_event(
            &events_subject,
            &PromptEvent::Done { stop_reason: "end_turn".to_string() },
        )
        .await;
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

/// Truncate a prompt to at most 256 characters for use as a session title.
fn truncate_title(text: &str) -> String {
    let sanitized = text.replace(['\r', '\n'], " ");
    let trimmed = sanitized.trim();
    if trimmed.len() <= 256 {
        trimmed.to_string()
    } else {
        format!("{}…", &trimmed[..255])
    }
}
