//! Handler: incident.io incident declared / updated / resolved.
//!
//! Triggered by NATS messages on `incidentio.incident.created`,
//! `incidentio.incident.updated`, `incidentio.incident.resolved`, etc.
//!
//! The agent gathers context about the incident and posts a structured
//! summary to Slack, then provides runbook guidance or resolution notes.
//!
//! # NATS payload
//! Raw JSON body forwarded from the incident.io webhook, e.g.:
//! ```json
//! {
//!   "event_type": "incident.created",
//!   "incident": {
//!     "id": "inc-123",
//!     "name": "API latency spike",
//!     "status": "triage",
//!     "severity": { "name": "P1" }
//!   }
//! }
//! ```

use serde_json::Value;
use tracing::{info, warn};

use super::{fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, slack};

/// Run the incident-response agent from a raw incident.io webhook payload.
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

    let event_type = event["event_type"].as_str().unwrap_or("unknown");
    let incident = &event["incident"];
    let incident_id = incident["id"].as_str().unwrap_or("unknown");
    let name = incident["name"].as_str().unwrap_or("(no name)");
    let status = incident["status"].as_str().unwrap_or("unknown");
    let severity = incident["severity"]["name"].as_str().unwrap_or("unknown");

    info!(
        incident_id,
        name, status, severity, event_type, "Starting incident.io handler"
    );

    let action_guidance = match event_type {
        "incident.created" => {
            "1. Post an immediate Slack message to the on-call channel with: incident name, \
             severity, status, and a link to incident.io.\n\
             2. List the most likely causes based on the incident name.\n\
             3. Suggest 3 concrete first-response steps the on-call engineer should take."
        }
        "incident.resolved" => {
            "1. Post a resolution message to the on-call Slack channel with: incident name, \
             resolution time, and a brief summary of what was done.\n\
             2. Remind the team to complete a post-mortem within 48 hours."
        }
        _ => {
            "1. Post a status update to the on-call Slack channel with the latest incident state.\n\
             2. Summarise what has changed since the incident was created."
        }
    };

    let prompt = format!(
        "You are an on-call assistant handling an incident.io event.\n\
         NATS subject: {nats_subject}\n\
         Event type: {event_type}\n\
         Incident ID: {incident_id}\n\
         Incident name: {name}\n\
         Status: {status}\n\
         Severity: {severity}\n\n\
         Full payload:\n```json\n{}\n```\n\n\
         {action_guidance}\n\
         Be concise and actionable.",
        serde_json::to_string_pretty(&event).unwrap_or_default()
    );

    let tools = incident_tools();

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
            info!(incident_id, event_type, "incident.io handler completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(incident_id, error = %e, "incident.io handler failed");
            Some(Err(e.to_string()))
        }
    }
}

fn incident_tools() -> Vec<ToolDef> {
    slack::slack_tool_defs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incident_tools_includes_slack() {
        let tools = incident_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"send_slack_message"), "slack tool missing");
        assert!(
            names.contains(&"read_slack_channel"),
            "read channel tool missing"
        );
    }

    fn make_agent(proxy_url: &str) -> AgentLoop {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let tool_ctx = Arc::new(ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: proxy_url.to_string(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: String::new(),
        });
        AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                reqwest::Client::new(),
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
        }
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        let agent = make_agent("http://127.0.0.1:1");
        let result = handle(&agent, "incidentio.incident.created", b"not json").await;
        assert!(matches!(result, Some(Err(_))));
    }

    #[tokio::test]
    async fn handle_incident_created_calls_agent() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("incident.created")
                .body_contains("API latency spike");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "incident handled"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {
                "id": "inc-001",
                "name": "API latency spike",
                "status": "triage",
                "severity": {"name": "P1"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_incident_resolved_calls_agent() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("incident.resolved");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "resolution noted"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.resolved",
            "incident": {
                "id": "inc-001",
                "name": "API latency spike",
                "status": "resolved",
                "severity": {"name": "P1"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.resolved", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_uses_fallback_values_when_fields_absent() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("unknown");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "handled"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let result = handle(&agent, "incidentio.event", b"{}").await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_returns_some_err_when_agent_fails() {
        // When the Anthropic API returns an error, run_agent fails and the
        // handler must return Some(Err(...)) rather than panicking or returning None.
        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("internal error");
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {"id": "inc-fail", "name": "test", "status": "active", "severity": {"name": "P1"}}
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(
            matches!(result, Some(Err(_))),
            "agent failure must produce Some(Err), got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn handle_malformed_severity_uses_unknown_fallback() {
        // When severity is a plain string rather than {"name": "..."}, as_str()
        // on severity["name"] returns None → fallback "unknown" is used.
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("unknown"); // severity fallback
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "handled"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {
                "id": "inc-sev",
                "name": "Bad severity",
                "status": "active",
                "severity": "P1"  // string, not {"name": "P1"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_incident_updated_uses_generic_guidance() {
        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("incident.updated")
                .body_contains("status update");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "status noted"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.updated",
            "incident": {
                "id": "inc-upd-1",
                "name": "DB latency",
                "status": "investigating",
                "severity": {"name": "P2"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.updated", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_with_null_incident_fields_uses_fallbacks() {
        let server = httpmock::MockServer::start_async().await;
        // When status/severity/name are explicitly null the prompt must still
        // be built (using the fallback strings) — no panic.
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("unknown") // status fallback
                .body_contains("(no name)"); // name fallback
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "ok"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {
                "id": null,
                "name": null,
                "status": null,
                "severity": null
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(matches!(result, Some(Ok(_))));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_uses_memory_as_system_prompt_when_fetch_succeeds() {
        // When GitHub returns a valid memory file, handle() must forward its
        // decoded content as the system prompt to the Anthropic API.
        use crate::tools::ToolContext;
        use base64::Engine as _;
        use base64::engine::general_purpose;
        use std::sync::Arc;

        let server = httpmock::MockServer::start_async().await;

        // Mock: GitHub contents endpoint returns a base64-encoded memory file.
        let memory_content = "# Runbook\nFor P1 incidents: page the on-call engineer immediately.";
        let encoded = general_purpose::STANDARD.encode(memory_content.as_bytes());
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains(".trogon/memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "content": encoded }));
        });

        // Mock: Anthropic receives the memory content as a system prompt.
        let anthropic_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Runbook")
                .body_contains("P1 incidents");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "incident handled with memory"}]
                }));
        });

        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::DefaultToolDispatcher;
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext {
            http_client: http_client.clone(),
            proxy_url: server.base_url(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: String::new(),
            slack_token: String::new(),
        });
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
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
        };

        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {
                "id": "inc-mem-ok",
                "name": "API latency spike",
                "status": "triage",
                "severity": {"name": "P1"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(
            matches!(result, Some(Ok(_))),
            "expected Ok, got: {:?}",
            result
        );
        anthropic_mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_uses_custom_memory_path_from_agent() {
        // When the AgentLoop has a custom memory_path, the GitHub fetch must use
        // that path — not the default ".trogon/memory.md".
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use base64::Engine as _;
        use base64::engine::general_purpose;
        use std::sync::Arc;

        let server = httpmock::MockServer::start_async().await;

        let memory_content = "# Custom runbook\nUse the custom runbook for all incidents.";
        let encoded = general_purpose::STANDARD.encode(memory_content.as_bytes());

        // The custom path must appear in the GET request — not the default path.
        let custom_path_mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path_contains("custom/runbook.md");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(serde_json::json!({ "content": encoded }));
            })
            .await;

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Custom runbook");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "handled with custom memory"}]
                }));
        });

        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext {
            http_client: http_client.clone(),
            proxy_url: server.base_url(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: String::new(),
            slack_token: String::new(),
        });
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
            // Custom path — should override the DEFAULT_MEMORY_PATH.
            memory_path: Some("custom/runbook.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
        };

        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {
                "id": "inc-custom-path",
                "name": "Custom path test",
                "status": "triage",
                "severity": {"name": "P2"}
            }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(
            matches!(result, Some(Ok(_))),
            "expected Ok, got: {:?}",
            result
        );
        // Verify the custom path was fetched from GitHub.
        custom_path_mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_proceeds_without_memory_when_fetch_fails() {
        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path_contains("memory.md");
            then.status(404);
        });
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

        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext {
            http_client: http_client.clone(),
            proxy_url: server.base_url(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: String::new(),
            slack_token: String::new(),
        });
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
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
        };

        let payload = serde_json::json!({
            "event_type": "incident.created",
            "incident": {"id": "inc-9", "name": "test", "status": "triage", "severity": {"name": "P2"}}
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let result = handle(&agent, "incidentio.incident.created", &bytes).await;
        assert!(
            matches!(result, Some(Ok(_))),
            "expected Ok even without memory"
        );
        anthropic_mock.assert_async().await;
    }
}
