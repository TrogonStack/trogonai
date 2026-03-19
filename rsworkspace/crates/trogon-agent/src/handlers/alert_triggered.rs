//! Handler: Datadog alert triggered / recovered.
//!
//! Triggered by NATS messages on `datadog.alert` or `datadog.alert.recovered`.
//! The agent analyses the alert payload and posts a summary (e.g. via Slack).
//!
//! # NATS payload
//! Raw JSON body forwarded from the Datadog webhook, e.g.:
//! ```json
//! {
//!   "alert_transition": "Triggered",
//!   "alert_id": "123456789",
//!   "alert_title": "CPU usage is too high on web-01",
//!   "alert_metric": "system.cpu.user",
//!   "host": "web-01"
//! }
//! ```

use serde_json::Value;
use tracing::{info, warn};

use super::{fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, slack};

/// Run the alert-triage agent from a raw Datadog webhook payload.
///
/// Always handles the event — returns `Some(Ok)` on success, `Some(Err)` on failure.
pub async fn handle(
    agent: &AgentLoop,
    nats_subject: &str,
    payload: &[u8],
) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    let transition = event["alert_transition"].as_str().unwrap_or("unknown");
    let title = event["alert_title"].as_str().unwrap_or("(no title)");
    let alert_id = event["alert_id"].as_str().unwrap_or("unknown");
    let host = event["host"].as_str().unwrap_or("unknown");

    info!(
        alert_id,
        title, transition, host, "Starting Datadog alert handler"
    );

    let prompt = format!(
        "You are an on-call assistant handling a Datadog alert.\n\
         NATS subject: {nats_subject}\n\
         Alert transition: {transition}\n\
         Alert title: {title}\n\
         Alert ID: {alert_id}\n\
         Host: {host}\n\n\
         Full payload:\n```json\n{}\n```\n\n\
         1. Analyse the alert severity and likely cause based on the title and metrics.\n\
         2. If the alert is triggered, use `send_slack_message` to notify the on-call channel\n\
            with a clear summary: what triggered, on which host, and suggested next steps.\n\
         3. If the alert recovered, post a brief resolution message to the same channel.\n\
         Be concise and actionable.",
        serde_json::to_string_pretty(&event).unwrap_or_default()
    );

    let tools = alert_tools();

    let mem_path = agent
        .memory_path
        .as_deref()
        .unwrap_or(super::DEFAULT_MEMORY_PATH);
    let memory = match (&agent.memory_owner, &agent.memory_repo) {
        (Some(owner), Some(repo)) => fetch_memory(agent, owner, repo, mem_path).await,
        _ => None,
    };

    match run_agent(agent, prompt, tools, memory).await {
        Ok(text) => {
            info!(alert_id, transition, "Datadog alert handler completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(alert_id, error = %e, "Datadog alert handler failed");
            Some(Err(e.to_string()))
        }
    }
}

fn alert_tools() -> Vec<ToolDef> {
    slack::slack_tool_defs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_tools_includes_slack() {
        let tools = alert_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"send_slack_message"), "slack tool missing");
        assert!(
            names.contains(&"read_slack_channel"),
            "read channel tool missing"
        );
    }

    fn make_agent(proxy_url: &str) -> AgentLoop {
        use crate::tools::ToolContext;
        use std::sync::Arc;
        AgentLoop {
            http_client: reqwest::Client::new(),
            proxy_url: proxy_url.to_string(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client: reqwest::Client::new(),
                proxy_url: proxy_url.to_string(),
                github_token: String::new(),
                linear_token: String::new(),
                slack_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            split_client: None,
            tenant_id: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            thinking_budget: None,
            permission_checker: None,
        }
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        let agent = make_agent("http://127.0.0.1:1");
        let result = handle(&agent, "datadog.alert", b"not json").await;
        assert!(matches!(result, Some(Err(_))));
    }

    #[tokio::test]
    async fn handle_calls_agent_for_triggered_alert() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Triggered")
                .body_contains("web-01");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "alert handled"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "alert_transition": "Triggered",
            "alert_id": "1234",
            "alert_title": "CPU usage is too high",
            "host": "web-01"
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "datadog.alert", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_uses_fallback_values_when_fields_absent() {
        // Payload with none of the optional fields — all unwrap_or defaults fire.
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                // Defaults: transition="unknown", title="(no title)", id="unknown", host="unknown"
                .body_contains("unknown");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "handled"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        // Empty JSON object — no alert_transition, no title, no id, no host.
        let result = handle(&agent, "datadog.event", b"{}").await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_proceeds_without_memory_when_fetch_fails() {
        // Agent has memory_owner/repo set, but the proxy returns 404 — handler
        // must still call the Anthropic API (with None system prompt) rather than failing.
        let server = httpmock::MockServer::start_async().await;
        // Memory endpoint returns 404.
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(404);
        });
        // Anthropic endpoint succeeds — must be called even without memory.
        let anthropic_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "ok"}]
                }));
        });

        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        use std::sync::Arc;
        let http_client = reqwest::Client::new();
        let agent = AgentLoop {
            http_client: http_client.clone(),
            proxy_url: server.base_url(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client,
                proxy_url: server.base_url(),
                github_token: "tok_github_prod_test01".to_string(),
                linear_token: String::new(),
                slack_token: String::new(),
            }),
            memory_owner: Some("owner".to_string()),
            memory_repo: Some("repo".to_string()),
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            split_client: None,
            tenant_id: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            thinking_budget: None,
            permission_checker: None,
        };

        let payload = serde_json::json!({
            "alert_transition": "Triggered",
            "alert_id": "99",
            "alert_title": "Disk full",
            "host": "db-01"
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "datadog.alert", &bytes).await;
        assert!(
            matches!(result, Some(Ok(_))),
            "expected Ok even without memory: {result:?}"
        );
        anthropic_mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_calls_agent_for_recovered_alert() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Recovered");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovery noted"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "alert_transition": "Recovered",
            "alert_id": "5678",
            "alert_title": "CPU usage back to normal",
            "host": "web-01"
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "datadog.alert.recovered", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }
}
