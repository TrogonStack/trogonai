//! Handler: Linear issue triage.
//!
//! Triggered by a NATS message on `linear.Issue` (created action).  The agent
//! fetches the full issue, analyses it, and posts a triage comment.
//!
//! # NATS payload
//! The message body must be a JSON object matching the Linear webhook shape:
//! ```json
//! {
//!   "action": "create",
//!   "data": { "id": "ISS-123", "title": "...", "description": "..." },
//!   "type": "Issue"
//! }
//! ```

use serde_json::Value;
use tracing::{info, warn};

use super::{fetch_memory, run_agent};
use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, slack, tool_def};

/// Run the issue-triage agent from a raw Linear webhook payload.
///
/// Returns `None` when the event is not a newly created issue.
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    let action = event["action"].as_str().unwrap_or("");
    let event_type = event["type"].as_str().unwrap_or("");

    if action != "create" || event_type != "Issue" {
        info!(
            action,
            event_type, "Linear event not relevant for triage — skipping"
        );
        return None;
    }

    let issue_id = event["data"]["id"].as_str()?;
    let title = event["data"]["title"].as_str().unwrap_or("(no title)");

    info!(issue_id, title, "Starting issue triage agent");

    let prompt = format!(
        "You are a project manager doing issue triage.\n\
         A new Linear issue has been created: {issue_id} — \"{title}\".\n\
         1. Use `get_linear_comments` to recall any prior triage decisions on this issue.\n\
         2. Use `get_linear_issue` to fetch full details.\n\
         3. Analyse the issue: is the description clear? Is priority set correctly?\n\
            Are labels appropriate?\n\
         4. Use `post_linear_comment` to post a short triage note with your assessment\n\
            and any suggested next steps.\n\
         Be concise and constructive."
    );

    let tools = triage_tools();

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
            info!(issue_id, "Issue triage completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(issue_id, error = %e, "Issue triage agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn triage_tools() -> Vec<ToolDef> {
    let mut tools = vec![
        tool_def(
            "get_linear_issue",
            "Fetch a Linear issue by ID, including state, assignee, labels, and team.",
            serde_json::json!({
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id": { "type": "string", "description": "Linear issue ID (e.g. ISS-123)" }
                }
            }),
        ),
        tool_def(
            "get_linear_comments",
            "Get all comments on a Linear issue — use this to recall prior triage decisions.",
            serde_json::json!({
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id": { "type": "string" }
                }
            }),
        ),
        tool_def(
            "post_linear_comment",
            "Post a comment on a Linear issue.",
            serde_json::json!({
                "type": "object",
                "required": ["issue_id", "body"],
                "properties": {
                    "issue_id": { "type": "string" },
                    "body":     { "type": "string", "description": "Markdown comment body" }
                }
            }),
        ),
        tool_def(
            "update_linear_issue",
            "Update a Linear issue's state, assignee, or priority.",
            serde_json::json!({
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id":    { "type": "string" },
                    "state_id":    { "type": "string", "description": "New workflow state ID" },
                    "assignee_id": { "type": "string", "description": "New assignee user ID" },
                    "priority":    { "type": "integer", "description": "Priority 0-4 (0=none, 1=urgent)" }
                }
            }),
        ),
    ];
    tools.extend(slack::slack_tool_defs());
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_tools_has_four_entries() {
        assert_eq!(triage_tools().len(), 6);
    }

    #[test]
    fn triage_tools_includes_memory_tool() {
        let tools = triage_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"get_linear_comments"));
    }

    #[tokio::test]
    async fn handle_skips_non_create_action() {
        // We need a minimal AgentLoop — it won't be called but must compile.
        // Use a dummy to verify the early-return path without network calls.
        let payload = serde_json::json!({
            "action": "update",
            "type": "Issue",
            "data": { "id": "ISS-1", "title": "t" }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();

        // Build a stub AgentLoop (it must never be called for non-create actions).
        let result = handle(&make_stub_agent(), &bytes).await;
        assert!(result.is_none(), "update action should be skipped");
    }

    fn make_stub_agent() -> AgentLoop {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;
        let tool_ctx = Arc::new(ToolContext::for_test("http://localhost:9999", "", "", ""));
        AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                reqwest::Client::new(),
                "http://localhost:9999".to_string(),
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

    #[tokio::test]
    async fn handle_skips_non_issue_type() {
        let payload = serde_json::json!({
            "action": "create",
            "type": "Comment",
            "data": { "id": "c1" }
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        assert!(handle(&make_stub_agent(), &bytes).await.is_none());
    }

    #[tokio::test]
    async fn handle_returns_error_on_invalid_json() {
        let result = handle(&make_stub_agent(), b"not json").await;
        assert!(matches!(result, Some(Err(_))));
    }
}
