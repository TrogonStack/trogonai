use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use trogon_agent_core::agent_loop::{AgentError, AgentEvent, Message, PermissionChecker};
use trogon_agent_core::tools::ToolDef;

use crate::agent::GatewayConfig;

/// Abstraction over the LLM loop used by `TrogonAgent`.
///
/// Implementors must be `Clone` so that `TrogonAgent` can cheaply create a
/// per-session copy and apply session-level overrides (model, MCP tools,
/// permission checker, gateway) without touching the shared default.
#[async_trait(?Send)]
pub trait AgentRunner: Clone {
    /// Returns the active model identifier (owned to avoid Mutex-borrow issues).
    fn model(&self) -> String;

    /// Override the model for this runner.
    fn set_model(&mut self, model: String);

    /// Append MCP tool definitions and their dispatch entries.
    fn add_mcp_tools(
        &mut self,
        defs: Vec<ToolDef>,
        dispatch: Vec<(String, String, Arc<trogon_mcp::McpClient>)>,
    );

    /// Install a permission checker that gates every tool execution.
    fn set_permission_checker(&mut self, checker: Arc<dyn PermissionChecker>);

    /// Override proxy / token / extra-headers from a `GatewayConfig`.
    fn apply_gateway(&mut self, config: &GatewayConfig);

    /// Stream agent events while running the LLM tool-use loop.
    ///
    /// `steer_rx` receives mid-turn steering messages published to
    /// `{prefix}.session.{id}.agent.steer`.  Each message is appended as a
    /// `Text` block to the next tool-results user turn so the model sees the
    /// guidance before deciding its next action.
    async fn run_chat_streaming(
        &self,
        messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
        event_tx: mpsc::Sender<AgentEvent>,
        steer_rx: Option<mpsc::Receiver<String>>,
    ) -> Result<Vec<Message>, AgentError>;
}

// ── Real implementation ───────────────────────────────────────────────────────

#[async_trait(?Send)]
impl AgentRunner for trogon_agent_core::agent_loop::AgentLoop {
    fn model(&self) -> String {
        self.model.clone()
    }

    fn set_model(&mut self, model: String) {
        self.model = model;
    }

    fn add_mcp_tools(
        &mut self,
        defs: Vec<ToolDef>,
        dispatch: Vec<(String, String, Arc<trogon_mcp::McpClient>)>,
    ) {
        self.mcp_tool_defs.extend(defs);
        self.mcp_dispatch.extend(dispatch);
    }

    fn set_permission_checker(&mut self, checker: Arc<dyn PermissionChecker>) {
        self.permission_checker = Some(checker);
    }

    fn apply_gateway(&mut self, config: &GatewayConfig) {
        self.anthropic_base_url = Some(config.base_url.clone());
        self.anthropic_token = config.token.clone();
        self.anthropic_extra_headers = config.extra_headers.clone();
    }

    async fn run_chat_streaming(
        &self,
        messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
        event_tx: mpsc::Sender<AgentEvent>,
        steer_rx: Option<mpsc::Receiver<String>>,
    ) -> Result<Vec<Message>, AgentError> {
        trogon_agent_core::agent_loop::AgentLoop::run_chat_streaming(
            self,
            messages,
            tools,
            system_prompt,
            event_tx,
            steer_rx,
        )
        .await
    }
}

// ── Mock (test-helpers feature) ───────────────────────────────────────────────

#[cfg(feature = "test-helpers")]
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    /// Scriptable `AgentRunner` for unit tests.
    ///
    /// By default `run_chat_streaming` returns the messages it received
    /// unchanged (no new turns).  Override with `with_response` or
    /// `with_error` to control what the mock returns.
    ///
    /// Steer messages received via `steer_rx` are always drained and stored;
    /// retrieve them with `captured_steer()`.
    #[derive(Clone)]
    pub struct MockAgentRunner {
        pub model: String,
        /// If `Some`, `run_chat_streaming` returns these messages.
        response: Arc<Mutex<Option<Vec<Message>>>>,
        /// If `Some`, `run_chat_streaming` emits these events before finishing.
        events: Arc<Mutex<Vec<AgentEvent>>>,
        /// If `Some`, `run_chat_streaming` returns this error string.
        error: Arc<Mutex<Option<AgentError>>>,
        /// Steer messages received via `steer_rx` during the last run.
        captured_steer: Arc<Mutex<Vec<String>>>,
    }

    impl MockAgentRunner {
        pub fn new(model: impl Into<String>) -> Self {
            Self {
                model: model.into(),
                response: Arc::new(Mutex::new(None)),
                events: Arc::new(Mutex::new(Vec::new())),
                error: Arc::new(Mutex::new(None)),
                captured_steer: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Provide a fixed response for `run_chat_streaming`.
        pub fn with_response(self, messages: Vec<Message>) -> Self {
            *self.response.lock().unwrap() = Some(messages);
            self
        }

        /// Emit these events before the runner finishes.
        pub fn with_events(self, events: Vec<AgentEvent>) -> Self {
            *self.events.lock().unwrap() = events;
            self
        }

        /// Make `run_chat_streaming` return an error.
        pub fn with_error(self, error: AgentError) -> Self {
            *self.error.lock().unwrap() = Some(error);
            self
        }

        /// Return all steer messages received during the last `run_chat_streaming` call.
        pub fn captured_steer(&self) -> Vec<String> {
            self.captured_steer.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait(?Send)]
    impl AgentRunner for MockAgentRunner {
        fn model(&self) -> String {
            self.model.clone()
        }

        fn set_model(&mut self, model: String) {
            self.model = model;
        }

        fn add_mcp_tools(
            &mut self,
            _defs: Vec<ToolDef>,
            _dispatch: Vec<(String, String, Arc<trogon_mcp::McpClient>)>,
        ) {
        }

        fn set_permission_checker(&mut self, _checker: Arc<dyn PermissionChecker>) {}

        fn apply_gateway(&mut self, _config: &GatewayConfig) {}

        async fn run_chat_streaming(
            &self,
            messages: Vec<Message>,
            _tools: &[ToolDef],
            _system_prompt: Option<&str>,
            event_tx: mpsc::Sender<AgentEvent>,
            mut steer_rx: Option<mpsc::Receiver<String>>,
        ) -> Result<Vec<Message>, AgentError> {
            // Drain any steer messages that are already buffered in the channel.
            if let Some(ref mut rx) = steer_rx {
                while let Ok(msg) = rx.try_recv() {
                    self.captured_steer.lock().unwrap().push(msg);
                }
            }
            let events = self.events.lock().unwrap().clone();
            for event in events {
                let _ = event_tx.send(event).await;
            }
            if let Some(error) = self.error.lock().unwrap().take() {
                return Err(error);
            }
            Ok(self
                .response
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(messages))
        }
    }
}
