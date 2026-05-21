use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use trogon_agent_core::agent_loop::{AgentError, AgentEvent, ElicitationProvider, Message, PermissionChecker};
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

    /// Override the working directory used by filesystem tools.
    fn set_cwd(&mut self, cwd: String);

    /// Append MCP tool definitions and their dispatch entries.
    fn add_mcp_tools(
        &mut self,
        defs: Vec<ToolDef>,
        dispatch: Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
    );

    /// Install a permission checker that gates every tool execution.
    fn set_permission_checker(&mut self, checker: Arc<dyn PermissionChecker>);

    /// Install an elicitation provider for the built-in `ask_user` tool.
    fn set_elicitation_provider(&mut self, provider: Arc<dyn ElicitationProvider>);

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

    fn set_cwd(&mut self, cwd: String) {
        self.tool_context = Arc::new(trogon_tools::ToolContext {
            proxy_url: self.tool_context.proxy_url.clone(),
            cwd,
            http_client: self.tool_context.http_client.clone(),
        });
    }

    fn add_mcp_tools(
        &mut self,
        defs: Vec<ToolDef>,
        dispatch: Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
    ) {
        self.mcp_tool_defs.extend(defs);
        self.mcp_dispatch.extend(dispatch);
    }

    fn set_permission_checker(&mut self, checker: Arc<dyn PermissionChecker>) {
        self.permission_checker = Some(checker);
    }

    fn set_elicitation_provider(&mut self, provider: Arc<dyn ElicitationProvider>) {
        self.elicitation_provider = Some(provider);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use trogon_tools::ToolContext;
    use trogon_agent_core::agent_loop::AgentLoop;
    use crate::agent::GatewayConfig;
    use super::AgentRunner;

    fn make_agent_loop() -> AgentLoop {
        AgentLoop {
            http_client: reqwest::Client::new(),
            proxy_url: String::new(),
            anthropic_token: "original-token".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            streaming_client: None,
            model: "claude-opus-4-6".to_string(),
            max_iterations: 10,
            thinking_budget: None,
            tool_context: Arc::new(ToolContext {
                proxy_url: String::new(),
                cwd: String::new(),
                http_client: reqwest::Client::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    #[test]
    fn apply_gateway_sets_base_url_token_and_headers() {
        let mut runner = make_agent_loop();
        let config = GatewayConfig {
            base_url: "https://proxy.example.com".to_string(),
            token: "gateway-token".to_string(),
            extra_headers: vec![("X-Custom".to_string(), "value".to_string())],
        };
        runner.apply_gateway(&config);
        assert_eq!(runner.anthropic_base_url, Some("https://proxy.example.com".to_string()));
        assert_eq!(runner.anthropic_token, "gateway-token");
        assert_eq!(runner.anthropic_extra_headers, vec![("X-Custom".to_string(), "value".to_string())]);
    }

    #[test]
    fn apply_gateway_overwrites_previous_values() {
        let mut runner = make_agent_loop();
        runner.apply_gateway(&GatewayConfig {
            base_url: "https://first.example.com".to_string(),
            token: "token-1".to_string(),
            extra_headers: vec![],
        });
        runner.apply_gateway(&GatewayConfig {
            base_url: "https://second.example.com".to_string(),
            token: "token-2".to_string(),
            extra_headers: vec![("X-New".to_string(), "v2".to_string())],
        });
        assert_eq!(runner.anthropic_base_url, Some("https://second.example.com".to_string()));
        assert_eq!(runner.anthropic_token, "token-2");
        assert_eq!(runner.anthropic_extra_headers, vec![("X-New".to_string(), "v2".to_string())]);
    }

    #[test]
    fn set_model_updates_model_field() {
        let mut runner = make_agent_loop();
        assert_eq!(runner.model(), "claude-opus-4-6");
        runner.set_model("claude-haiku-4-5".to_string());
        assert_eq!(runner.model(), "claude-haiku-4-5");
    }

    #[test]
    fn add_mcp_tools_extends_tool_defs() {
        let mut runner = make_agent_loop();
        assert!(runner.mcp_tool_defs.is_empty());
        let def = trogon_tools::tool_def("my_tool", "does stuff", serde_json::json!({"type":"object","properties":{}}));
        runner.add_mcp_tools(vec![def], vec![]);
        assert_eq!(runner.mcp_tool_defs.len(), 1);
        assert_eq!(runner.mcp_tool_defs[0].name, "my_tool");
    }

    #[test]
    fn add_mcp_tools_accumulates_across_calls() {
        let mut runner = make_agent_loop();
        runner.add_mcp_tools(
            vec![trogon_tools::tool_def("tool_a", "a", serde_json::json!({}))],
            vec![],
        );
        runner.add_mcp_tools(
            vec![trogon_tools::tool_def("tool_b", "b", serde_json::json!({}))],
            vec![],
        );
        assert_eq!(runner.mcp_tool_defs.len(), 2);
        assert_eq!(runner.mcp_tool_defs[0].name, "tool_a");
        assert_eq!(runner.mcp_tool_defs[1].name, "tool_b");
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
    ///
    /// For NATS integration tests where steer messages arrive asynchronously,
    /// use `with_steer_wait()`: the runner calls `recv().await` for the first
    /// steer message, and `with_started_notify()` to signal when it is ready.
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
        /// Notified when `run_chat_streaming` begins (before any steer wait).
        started_notify: Arc<tokio::sync::Notify>,
        /// If true, await `steer_rx.recv()` for one message before returning.
        wait_for_steer: bool,
        /// Names of all tool defs passed to `add_mcp_tools` across all calls.
        recorded_tool_names: Arc<Mutex<Vec<String>>>,
        /// Set to true when `set_elicitation_provider` is called.
        pub elicitation_provider_set: Arc<Mutex<bool>>,
        /// Set to true when `set_permission_checker` is called.
        pub permission_checker_set: Arc<Mutex<bool>>,
        /// Captured permission checker for calling during `run_chat_streaming`.
        captured_permission_checker: Arc<Mutex<Option<Arc<dyn PermissionChecker>>>>,
        /// When `Some`, `run_chat_streaming` calls the captured permission checker
        /// with this (tool_name, tool_input) before returning, simulating a tool call.
        invoke_checker_for_tool: Arc<Mutex<Option<(String, serde_json::Value)>>>,
        /// The last `system_prompt` argument passed to `run_chat_streaming`.
        captured_system_prompt: Arc<Mutex<Option<String>>>,
    }

    impl MockAgentRunner {
        pub fn new(model: impl Into<String>) -> Self {
            Self {
                model: model.into(),
                response: Arc::new(Mutex::new(None)),
                events: Arc::new(Mutex::new(Vec::new())),
                error: Arc::new(Mutex::new(None)),
                captured_steer: Arc::new(Mutex::new(Vec::new())),
                started_notify: Arc::new(tokio::sync::Notify::new()),
                wait_for_steer: false,
                recorded_tool_names: Arc::new(Mutex::new(Vec::new())),
                elicitation_provider_set: Arc::new(Mutex::new(false)),
                permission_checker_set: Arc::new(Mutex::new(false)),
                captured_permission_checker: Arc::new(Mutex::new(None)),
                invoke_checker_for_tool: Arc::new(Mutex::new(None)),
                captured_system_prompt: Arc::new(Mutex::new(None)),
            }
        }

        /// Configure the mock to call the permission checker with `(tool, input)`
        /// during `run_chat_streaming`, simulating a tool-use permission gate.
        ///
        /// Only takes effect if a permission checker has been injected via
        /// `set_permission_checker`. The tool should be in `allowed_tools` to
        /// avoid blocking on the interactive channel.
        pub fn with_permission_check(self, tool: impl Into<String>, input: serde_json::Value) -> Self {
            *self.invoke_checker_for_tool.lock().unwrap() = Some((tool.into(), input));
            self
        }

        /// Return the names of all tool defs injected via `add_mcp_tools`.
        pub fn captured_tool_names(&self) -> Vec<String> {
            self.recorded_tool_names.lock().unwrap().clone()
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

        /// Share a `Notify` that is fired when `run_chat_streaming` starts.
        ///
        /// Use this in integration tests to know when it is safe to publish
        /// a steer message — i.e. the steer subscription is active and the
        /// runner is waiting for input.
        pub fn with_started_notify(mut self, notify: Arc<tokio::sync::Notify>) -> Self {
            self.started_notify = notify;
            self
        }

        /// Wait for one steer message with `recv().await` before finishing.
        ///
        /// Enables NATS integration tests where the steer message arrives
        /// asynchronously after the runner has started.
        pub fn with_steer_wait(mut self) -> Self {
            self.wait_for_steer = true;
            self
        }

        /// Return all steer messages received during the last `run_chat_streaming` call.
        pub fn captured_steer(&self) -> Vec<String> {
            self.captured_steer.lock().unwrap().clone()
        }

        /// Return the `system_prompt` argument from the last `run_chat_streaming` call.
        pub fn captured_system_prompt(&self) -> Option<String> {
            self.captured_system_prompt.lock().unwrap().clone()
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

        fn set_cwd(&mut self, _cwd: String) {}

        fn add_mcp_tools(
            &mut self,
            defs: Vec<ToolDef>,
            _dispatch: Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
        ) {
            let mut recorded = self.recorded_tool_names.lock().unwrap();
            for def in &defs {
                recorded.push(def.name.clone());
            }
        }

        fn set_permission_checker(&mut self, checker: Arc<dyn PermissionChecker>) {
            *self.permission_checker_set.lock().unwrap() = true;
            *self.captured_permission_checker.lock().unwrap() = Some(checker);
        }

        fn set_elicitation_provider(&mut self, _provider: Arc<dyn ElicitationProvider>) {
            *self.elicitation_provider_set.lock().unwrap() = true;
        }

        fn apply_gateway(&mut self, _config: &GatewayConfig) {}

        async fn run_chat_streaming(
            &self,
            messages: Vec<Message>,
            _tools: &[ToolDef],
            system_prompt: Option<&str>,
            event_tx: mpsc::Sender<AgentEvent>,
            mut steer_rx: Option<mpsc::Receiver<String>>,
        ) -> Result<Vec<Message>, AgentError> {
            // If configured, invoke the captured permission checker with a fake tool
            // call to exercise the audit recording path.
            if let Some((tool, input)) = self.invoke_checker_for_tool.lock().unwrap().clone() {
                if let Some(checker) = self.captured_permission_checker.lock().unwrap().clone() {
                    let _ = checker.check("mock-tc-1", &tool, &input).await;
                }
            }

            *self.captured_system_prompt.lock().unwrap() = system_prompt.map(str::to_string);
            // Signal that the runner has started and the steer subscription is live.
            self.started_notify.notify_one();

            // If configured, wait for the first steer message asynchronously.
            // This is used by NATS integration tests where the message arrives
            // after the runner starts rather than being pre-injected.
            if self.wait_for_steer
                && let Some(ref mut rx) = steer_rx
                && let Some(msg) = rx.recv().await
            {
                self.captured_steer.lock().unwrap().push(msg);
            }

            // Drain any remaining buffered steer messages.
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
