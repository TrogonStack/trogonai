//! NATS event handlers — one module per integration.
//!
//! Each handler receives a raw NATS message payload, extracts the relevant
//! context, runs the agentic loop, and returns the final model output.

pub mod ci_completed;
pub mod comment_added;
pub mod issue_triage;
pub mod pr_merged;
pub mod pr_review;
pub mod push_to_branch;

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose};
use tracing::debug;

use crate::agent_loop::{AgentLoop, AgentError, Message};
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
        agent.proxy_url,
    );

    let response = agent
        .http_client
        .get(&url)
        .header("Authorization", format!("Bearer {}", agent.tool_context.github_token))
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        debug!(owner, repo, status = %response.status(), ".trogon/memory.md not found — starting without memory");
        return None;
    }

    let body: serde_json::Value = response.json().await.ok()?;
    let raw = body["content"].as_str()?.replace('\n', "");
    let bytes = general_purpose::STANDARD.decode(&raw).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use httpmock::MockServer;

    fn make_agent(proxy_url: &str) -> AgentLoop {
        let http_client = reqwest::Client::new();
        AgentLoop {
            http_client: http_client.clone(),
            proxy_url: proxy_url.to_string(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client,
                proxy_url: proxy_url.to_string(),
                github_token: "tok_github_prod_test01".to_string(),
                linear_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        }
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
        let mock = server.mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("custom/notes.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "content": raw }));
        }).await;

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
}

/// Convenience: build the shared [`ToolContext`] that all handlers share.
pub fn make_tool_context(
    http_client: reqwest::Client,
    proxy_url: String,
    github_token: String,
    linear_token: String,
) -> Arc<ToolContext> {
    Arc::new(ToolContext { http_client, proxy_url, github_token, linear_token })
}
