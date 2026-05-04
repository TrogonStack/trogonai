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

    fn make_agent() -> AgentLoop {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![])),
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
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    fn end_turn(text: &str) -> serde_json::Value {
        serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": text}]
        })
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        let agent = make_agent();
        let result = handle(&agent, "incidentio.incident.created", b"not json").await;
        assert!(matches!(result, Some(Err(_))));
    }

    #[tokio::test]
    async fn handle_incident_created_calls_agent() {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("incident handled"),
            ])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
    }

    #[tokio::test]
    async fn handle_incident_resolved_calls_agent() {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("resolution noted"),
            ])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
    }

    #[tokio::test]
    async fn handle_uses_fallback_values_when_fields_absent() {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("handled"),
            ])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = handle(&agent, "incidentio.event", b"{}").await;
        assert!(matches!(result, Some(Ok(_))));
    }

    #[tokio::test]
    async fn handle_returns_some_err_when_agent_fails() {
        // When the Anthropic client returns an error, run_agent fails and the
        // handler must return Some(Err(...)) rather than panicking or returning None.
        use crate::agent_loop::mock::AlwaysErrAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(AlwaysErrAnthropicClient),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("handled"),
            ])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
    }

    #[tokio::test]
    async fn handle_incident_updated_uses_generic_guidance() {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("status noted"),
            ])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
    }

    #[tokio::test]
    async fn handle_with_null_incident_fields_uses_fallbacks() {
        // When status/severity/name are explicitly null the prompt must still
        // be built (using the fallback strings) — no panic.
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![end_turn("ok")])),
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
            permission_checker: None,
            elicitation_provider: None,
        };

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
    }

    #[tokio::test]
    async fn handle_uses_memory_as_system_prompt_when_fetch_succeeds() {
        // When the config returns a valid memory file, handle() must proceed normally.
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::mock::{MockAgentConfig, MockToolDispatcher};
        use base64::Engine as _;
        use base64::engine::general_purpose;
        use std::sync::Arc;

        let memory_content = "# Runbook\nFor P1 incidents: page the on-call engineer immediately.";
        let encoded = general_purpose::STANDARD.encode(memory_content.as_bytes());
        let tool_ctx = Arc::new(MockAgentConfig {
            github_contents: Some(serde_json::json!({ "content": encoded })),
            ..Default::default()
        });
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("incident handled with memory"),
            ])),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: tool_ctx,
            memory_owner: Some("owner".to_string()),
            memory_repo: Some("repo".to_string()),
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
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
    }

    #[tokio::test]
    async fn handle_uses_custom_memory_path_from_agent() {
        // When the AgentLoop has a custom memory_path, the GitHub fetch must use
        // that path — not the default ".trogon/memory.md".
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::mock::{MockAgentConfig, MockToolDispatcher};
        use base64::Engine as _;
        use base64::engine::general_purpose;
        use std::sync::Arc;

        let memory_content = "# Custom runbook\nUse the custom runbook for all incidents.";
        let encoded = general_purpose::STANDARD.encode(memory_content.as_bytes());

        let cfg = Arc::new(MockAgentConfig {
            github_contents: Some(serde_json::json!({ "content": encoded })),
            ..Default::default()
        });
        let tool_context: Arc<dyn crate::tools::AgentConfig> = cfg.clone();
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                end_turn("handled with custom memory"),
            ])),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context,
            memory_owner: Some("owner".to_string()),
            memory_repo: Some("repo".to_string()),
            // Custom path — should override the DEFAULT_MEMORY_PATH.
            memory_path: Some("custom/runbook.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
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
        // Verify the custom path was forwarded to the config layer's fetch call.
        let url = cfg.last_fetched_url().expect("fetch_github_contents was not called");
        assert!(url.contains("custom/runbook.md"), "URL {url:?} does not contain the custom path");
    }

    #[tokio::test]
    async fn handle_proceeds_without_memory_when_fetch_fails() {
        // Config returns None → fetch_memory returns None, handler must still run.
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::mock::{MockAgentConfig, MockToolDispatcher};
        use std::sync::Arc;

        let tool_ctx = Arc::new(MockAgentConfig { github_contents: None, ..Default::default() });
        let agent = AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![end_turn("ok")])),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: tool_ctx,
            memory_owner: Some("owner".to_string()),
            memory_repo: Some("repo".to_string()),
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
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
    }
}
