//! Handler: comment added on a pull request or issue.
//!
//! Triggered by a `issue_comment` webhook event.  The agent reads the comment,
//! checks whether it's a question or action request directed at the bot, and
//! responds accordingly.

use serde_json::Value;
use tracing::{info, warn};

use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, tool_def};
use super::{fetch_memory, run_agent, DEFAULT_MEMORY_PATH};

/// Run the comment-added agent from a raw GitHub `issue_comment` webhook payload.
///
/// Returns `None` when the event action is not `"created"`.
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    if event["action"].as_str() != Some("created") {
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let issue_number = event["issue"]["number"].as_u64()?;
    let comment_body = event["comment"]["body"].as_str().unwrap_or("");
    let commenter = event["comment"]["user"]["login"].as_str().unwrap_or("unknown");
    let is_pr = event["issue"]["pull_request"].is_object();

    let kind = if is_pr { "pull request" } else { "issue" };

    info!(owner, repo, issue_number, commenter, "Starting comment-added agent");

    let prompt = format!(
        "{commenter} commented on {kind} #{issue_number} in {owner}/{repo}:\n\
         \"{comment_body}\"\n\n\
         1. Use `get_pr_comments` to read the full discussion context.\n\
         2. Determine if this comment needs a response (question, request, confusion).\n\
         3. If a response is warranted, post a helpful reply using `post_pr_comment`.\n\
         4. If not, do nothing — not every comment needs a bot reply.\n\
         Be concise and only respond when genuinely useful."
    );

    let mem_path = agent.memory_path.as_deref().unwrap_or(DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, comment_tools(), memory).await {
        Ok(text) => {
            info!(issue_number, "Comment-added agent completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(issue_number, error = %e, "Comment-added agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn comment_tools() -> Vec<ToolDef> {
    vec![
        tool_def("get_pr_comments", "Get all comments on a pull request or issue.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"}
                }
            }),
        ),
        tool_def("post_pr_comment", "Post a comment on a pull request or issue.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","pr_number","body"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "pr_number": {"type":"integer"},
                    "body": {"type":"string"}
                }
            }),
        ),
        tool_def("get_file_contents",
            "Read a file from the repository. Returns JSON with `sha` and `content`.",
            serde_json::json!({
                "type": "object", "required": ["owner","repo","path"],
                "properties": {
                    "owner": {"type":"string"}, "repo": {"type":"string"},
                    "path": {"type":"string"}, "ref": {"type":"string"}
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_tools_has_three_entries() {
        assert_eq!(comment_tools().len(), 3);
    }

    #[tokio::test]
    async fn handle_skips_non_created_action() {
        use std::sync::Arc;
        use crate::agent_loop::AgentLoop;
        use crate::tools::ToolContext;
        let agent = AgentLoop {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:9999".to_string(),
            anthropic_token: String::new(),
            model: "test".to_string(),
            max_iterations: 1,
            tool_context: Arc::new(ToolContext {
                http_client: reqwest::Client::new(),
                proxy_url: "http://localhost:9999".to_string(),
                github_token: String::new(),
                linear_token: String::new(),
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
        };
        let payload = serde_json::json!({
            "action": "deleted",
            "issue": {"number": 1, "pull_request": {}},
            "comment": {"body": "hi", "user": {"login": "u"}},
            "repository": {"owner": {"login": "o"}, "name": "r"}
        });
        assert!(handle(&agent, &serde_json::to_vec(&payload).unwrap()).await.is_none());
    }
}
