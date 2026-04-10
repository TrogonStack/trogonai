//! NATS event handlers — one module per integration.
//!
//! Each handler receives a raw NATS message payload, extracts the relevant
//! context, runs the agentic loop, and returns the final model output.

pub mod alert_triggered;
pub mod ci_completed;
pub mod comment_added;
pub mod incident_declared;
pub mod issue_triage;
pub mod pr_merged;
pub mod pr_review;
pub mod push_to_branch;

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose};
use tracing::debug;

use crate::agent_loop::{AgentError, AgentLoop, Message};
use crate::tools::{ToolContext, ToolDef};

/// Common entry point used by both handlers.
///
/// `system_prompt` is forwarded to the Anthropic `system` field — pass the
/// contents of `.trogon/memory.md` here to give the agent persistent memory.
///
/// Returns the text produced by the model.
pub async fn run_agent(
    agent: &AgentLoop,
    prompt: String,
    tools: Vec<ToolDef>,
    system_prompt: Option<String>,
) -> Result<String, AgentError> {
    let messages = vec![Message::user_text(prompt)];
    agent.run(messages, &tools, system_prompt.as_deref()).await
}

/// Default memory file path used when none is configured.
pub const DEFAULT_MEMORY_PATH: &str = ".trogon/memory.md";

/// Fetch the agent memory file from a GitHub repository via the proxy.
///
/// `path` is the file path inside the repo (e.g. `.trogon/memory.md`).
/// Returns `Some(content)` when the file exists and is valid UTF-8.
/// Returns `None` silently on 404 (file not yet created) or any error — the
/// agent continues without a system prompt rather than failing.
pub async fn fetch_memory(
    agent: &AgentLoop,
    owner: &str,
    repo: &str,
    path: &str,
) -> Option<String> {
    let url = format!(
        "{}/github/repos/{owner}/{repo}/contents/{path}",
        agent.tool_context.proxy_url(),
    );

    let token = agent.tool_context.github_token().to_string();
    let body = agent
        .tool_context
        .fetch_github_contents(&url, &token)
        .await?;

    let raw = body["content"].as_str()?.replace('\n', "");
    let bytes = general_purpose::STANDARD.decode(&raw).ok()?;
    String::from_utf8(bytes).ok()
}

/// Convenience: build the shared [`ToolContext`] that all handlers share.
pub fn make_tool_context(
    http_client: reqwest::Client,
    proxy_url: String,
    github_token: String,
    linear_token: String,
    slack_token: String,
) -> Arc<ToolContext<reqwest::Client>> {
    Arc::new(ToolContext::new(
        http_client,
        proxy_url,
        github_token,
        linear_token,
        slack_token,
    ))
}

/// Run a single automation against a raw NATS event payload.
///
/// - Prepends the NATS subject and raw event JSON to `automation.prompt` so
///   the model always has the full event context.
/// - If `automation.tools` is empty, all built-in tools are available;
///   otherwise only the named tools are included.
/// - `automation.memory_path` overrides the per-handler default; if both are
///   `None` the global [`DEFAULT_MEMORY_PATH`] is used.
pub async fn run_automation(
    agent: &AgentLoop,
    automation: &trogon_automations::Automation,
    nats_subject: &str,
    payload: &[u8],
) -> Result<String, String> {
    // Build the built-in tool list (optionally filtered by the automation config).
    let all = crate::tools::all_tool_defs();
    let mut tools: Vec<crate::tools::ToolDef> = if automation.tools.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|t| automation.tools.contains(&t.name))
            .collect()
    };

    // Initialise automation-specific MCP servers and extend the tool list.
    let auto_mcp_configs: Vec<crate::config::McpServerConfig> = automation
        .mcp_servers
        .iter()
        .map(|s| crate::config::McpServerConfig {
            name: s.name.clone(),
            url: s.url.clone(),
        })
        .collect();
    let (auto_mcp_defs, auto_mcp_dispatch) =
        agent.tool_context.init_mcp_clients(&auto_mcp_configs).await;
    tools.extend(auto_mcp_defs);

    // Build a temporary AgentLoop when the automation overrides model or MCP dispatch.
    let merged;
    let effective: &AgentLoop = if auto_mcp_dispatch.is_empty() && automation.model.is_none() {
        agent
    } else {
        let mut merged_dispatch = agent.mcp_dispatch.clone();
        merged_dispatch.extend(auto_mcp_dispatch);
        merged = AgentLoop {
            anthropic_client: Arc::clone(&agent.anthropic_client),
            model: automation
                .model
                .clone()
                .unwrap_or_else(|| agent.model.clone()),
            max_iterations: agent.max_iterations,
            tool_dispatcher: Arc::clone(&agent.tool_dispatcher),
            tool_context: Arc::clone(&agent.tool_context),
            memory_owner: agent.memory_owner.clone(),
            memory_repo: agent.memory_repo.clone(),
            memory_path: agent.memory_path.clone(),
            mcp_tool_defs: agent.mcp_tool_defs.clone(),
            mcp_dispatch: merged_dispatch,
            flag_client: Arc::clone(&agent.flag_client),
            tenant_id: agent.tenant_id.clone(),
            // Inherit the promise context so checkpointing continues to work
            // even when the automation overrides the model or MCP dispatch.
            promise_store: agent.promise_store.clone(),
            promise_id: agent.promise_id.clone(),
        };
        &merged
    };

    // Format the user prompt: event context + automation-specific instructions.
    let event_json = serde_json::from_slice::<serde_json::Value>(payload)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
        .unwrap_or_else(|_| String::from_utf8_lossy(payload).to_string());

    let full_prompt = format!(
        "Event subject: {nats_subject}\n\nEvent payload:\n```json\n{event_json}\n```\n\n{}",
        automation.prompt
    );

    // Resolve the memory path: automation override → agent default → built-in default.
    let mem_path = automation
        .memory_path
        .as_deref()
        .or(effective.memory_path.as_deref())
        .unwrap_or(DEFAULT_MEMORY_PATH);

    let memory = match (&effective.memory_owner, &effective.memory_repo) {
        (Some(owner), Some(repo)) => {
            if effective
                .is_flag_enabled(&crate::flags::AgentFlag::MemoryEnabled)
                .await
            {
                fetch_memory(effective, owner, repo, mem_path).await
            } else {
                debug!("Memory fetch skipped — agent_memory_enabled flag is off");
                None
            }
        }
        _ => None,
    };

    run_agent(effective, full_prompt, tools, memory)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use httpmock::MockServer;

    fn make_automation(tools: Vec<String>) -> trogon_automations::Automation {
        trogon_automations::Automation {
            id: "test-auto-1".to_string(),
            tenant_id: "acme".to_string(),
            name: "Test automation".to_string(),
            trigger: "github.push".to_string(),
            prompt: "Review this push.".to_string(),
            model: None,
            tools,
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn end_turn_json() -> serde_json::Value {
        serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "automation output"}]
        })
    }

    /// run_automation includes the NATS subject and event JSON in the prompt.
    #[tokio::test]
    async fn run_automation_prompt_includes_subject_and_payload() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Event subject: github.push")
                .body_contains("ref_name"); // key appears in the JSON-escaped prompt
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_json());
        });

        let agent = make_agent(&server.base_url());
        let automation = make_automation(vec![]);
        let payload = serde_json::to_vec(&serde_json::json!({"ref_name": "main"})).unwrap();
        let result = run_automation(&agent, &automation, "github.push", &payload).await;
        assert!(result.is_ok(), "expected Ok: {result:?}");
        assert_eq!(result.unwrap(), "automation output");
    }

    /// Empty tools list → all 14 built-in tools are forwarded to the model.
    #[tokio::test]
    async fn run_automation_empty_tools_sends_all_builtins() {
        let server = MockServer::start_async().await;
        // Both a GitHub tool and a Slack tool must be present when tools == all.
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("list_pr_files")
                .body_contains("read_slack_channel");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_json());
        });

        let agent = make_agent(&server.base_url());
        let automation = make_automation(vec![]); // empty = all tools
        let result = run_automation(&agent, &automation, "github.push", b"{}").await;
        assert!(result.is_ok(), "expected Ok: {result:?}");
        mock.assert_async().await;
    }

    /// Specific tools list → only named tools appear; others are excluded.
    #[tokio::test]
    async fn run_automation_specific_tools_filters_others() {
        let server = MockServer::start_async().await;
        // If "list_pr_files" appears in the request the filter is broken → return error.
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("list_pr_files");
            then.status(500)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({"error": "unexpected tool list_pr_files"}));
        });
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("get_pr_diff");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_json());
        });

        let agent = make_agent(&server.base_url());
        let automation = make_automation(vec![
            "get_pr_diff".to_string(),
            "post_pr_comment".to_string(),
        ]);
        let result = run_automation(&agent, &automation, "github.push", b"{}").await;
        assert!(
            result.is_ok(),
            "list_pr_files was incorrectly included: {result:?}"
        );
    }

    /// Automation memory_path overrides the agent's default memory path.
    #[tokio::test]
    async fn run_automation_memory_path_override_used() {
        let server = MockServer::start_async().await;
        // Mock: custom memory path fetch succeeds.
        let raw = general_purpose::STANDARD.encode("# Custom memory\n");
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("custom/notes.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "content": raw }));
        });
        // Mock: Anthropic responds (system prompt present = memory was fetched).
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Custom memory");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_json());
        });

        // Agent has memory_owner/repo set so fetch_memory is attempted.
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::DefaultToolDispatcher;
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(crate::tools::ToolContext::new(
            http_client.clone(),
            server.base_url(),
            "tok_github_prod_test01".to_string(),
            String::new(),
            String::new(),
        ));
        let agent = AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http_client,
                server.base_url(),
                String::new(),
            )),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: Some("owner".to_string()),
            memory_repo: Some("repo".to_string()),
            memory_path: Some(".trogon/memory.md".to_string()), // agent default
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
        };
        let mut automation = make_automation(vec![]);
        automation.memory_path = Some("custom/notes.md".to_string()); // override

        let result = run_automation(&agent, &automation, "github.push", b"{}").await;
        assert!(result.is_ok(), "expected Ok: {result:?}");
    }

    fn make_agent(proxy_url: &str) -> AgentLoop {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::DefaultToolDispatcher;
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext::new(
            http_client.clone(),
            proxy_url.to_string(),
            "tok_github_prod_test01".to_string(),
            String::new(),
            String::new(),
        ));
        AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http_client,
                proxy_url.to_string(),
                String::new(),
            )),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
        }
    }

    // ── MockAgentConfig-based fetch_memory tests (no network) ────────────────

    fn make_agent_with_mock_config(cfg: crate::tools::mock::MockAgentConfig) -> AgentLoop {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::mock::MockToolDispatcher;
        AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                reqwest::Client::new(),
                "http://unused.test".to_string(),
                String::new(),
            )),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: Arc::new(cfg),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
        }
    }

    /// MockAgentConfig returning None → fetch_memory returns None without HTTP.
    #[tokio::test]
    async fn fetch_memory_mock_returns_none_when_config_has_no_contents() {
        let cfg = crate::tools::mock::MockAgentConfig::default(); // github_contents: None
        let agent = make_agent_with_mock_config(cfg);
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// MockAgentConfig returning valid base64 JSON → fetch_memory decodes it.
    #[tokio::test]
    async fn fetch_memory_mock_decodes_base64_from_config() {
        use base64::engine::general_purpose;
        let encoded = general_purpose::STANDARD.encode("# Notes\nsome memory\n");
        let cfg = crate::tools::mock::MockAgentConfig {
            github_contents: Some(serde_json::json!({ "content": encoded })),
            ..Default::default()
        };
        let agent = make_agent_with_mock_config(cfg);
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert_eq!(result.as_deref(), Some("# Notes\nsome memory\n"));
    }

    /// MockAgentConfig returning JSON without a `content` field → returns None.
    #[tokio::test]
    async fn fetch_memory_mock_returns_none_when_content_field_absent() {
        let cfg = crate::tools::mock::MockAgentConfig {
            github_contents: Some(serde_json::json!({ "sha": "abc123" })),
            ..Default::default()
        };
        let agent = make_agent_with_mock_config(cfg);
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// 404 response → fetch_memory returns None.
    #[tokio::test]
    async fn fetch_memory_returns_none_on_404() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(404);
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// 200 with valid base64 content → fetch_memory returns the decoded string.
    #[tokio::test]
    async fn fetch_memory_decodes_base64_content() {
        let server = MockServer::start_async().await;

        let raw = general_purpose::STANDARD.encode("# Memory\nAgent notes.\n");
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "content": raw }));
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert_eq!(result.as_deref(), Some("# Memory\nAgent notes.\n"));
    }

    /// Custom memory path is used in the URL.
    #[tokio::test]
    async fn fetch_memory_uses_custom_path() {
        let server = MockServer::start_async().await;
        let raw = general_purpose::STANDARD.encode("custom memory");
        let mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path_contains("custom/notes.md");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(serde_json::json!({ "content": raw }));
            })
            .await;

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", "custom/notes.md").await;
        assert_eq!(result.as_deref(), Some("custom memory"));
        mock.assert_async().await;
    }

    /// Network error → fetch_memory returns None rather than propagating error.
    #[tokio::test]
    async fn fetch_memory_returns_none_on_network_error() {
        // Point at a port where nothing listens.
        let agent = make_agent("http://127.0.0.1:1");
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// 403 Forbidden → fetch_memory returns None (not found / no access).
    #[tokio::test]
    async fn fetch_memory_returns_none_on_403() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(403);
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// 500 Internal Server Error → fetch_memory returns None gracefully.
    #[tokio::test]
    async fn fetch_memory_returns_none_on_500() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(500);
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// Response body where `content` field is absent → fetch_memory returns None.
    #[tokio::test]
    async fn fetch_memory_returns_none_when_content_field_missing() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "name": "memory.md", "sha": "abc" }));
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }

    /// Response body where `content` is invalid base64 → fetch_memory returns None.
    #[tokio::test]
    async fn fetch_memory_returns_none_on_invalid_base64() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "content": "!!!not-valid-base64!!!" }));
        });

        let agent = make_agent(&server.base_url());
        let result = fetch_memory(&agent, "owner", "repo", DEFAULT_MEMORY_PATH).await;
        assert!(result.is_none());
    }
}
