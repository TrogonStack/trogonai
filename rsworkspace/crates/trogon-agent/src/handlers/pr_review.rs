//! Handler: automated PR review.
//!
//! Triggered by a NATS message on `github.pull_request` (created / reopened /
//! synchronize actions).  The agent fetches the diff and changed files, then
//! posts a review comment back to the PR.
//!
//! # NATS payload
//! The message body must be a JSON object with at minimum:
//! ```json
//! {
//!   "action": "opened",
//!   "number": 42,
//!   "repository": { "owner": { "login": "org" }, "name": "repo" }
//! }
//! ```
//! This is the standard GitHub `pull_request` webhook payload shape.

use serde_json::Value;
use tracing::{info, warn};

use crate::agent_loop::AgentLoop;
use crate::tools::{ToolDef, tool_def};
use super::run_agent;

/// Actions that trigger a review.
const REVIEW_ACTIONS: &[&str] = &["opened", "reopened", "synchronize"];

/// Run the PR-review agent from a raw GitHub `pull_request` webhook payload.
///
/// Returns `None` when the action is not relevant (e.g. "closed", "labeled").
pub async fn handle(agent: &AgentLoop, payload: &[u8]) -> Option<Result<String, String>> {
    let event: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("JSON parse error: {e}"))),
    };

    let action = event["action"].as_str().unwrap_or("");
    if !REVIEW_ACTIONS.contains(&action) {
        info!(action, "PR action not relevant for review — skipping");
        return None;
    }

    let owner = event["repository"]["owner"]["login"].as_str()?;
    let repo = event["repository"]["name"].as_str()?;
    let pr_number = event["number"].as_u64()?;

    info!(owner, repo, pr_number, "Starting PR review agent");

    let prompt = format!(
        "You are a code reviewer. Review the pull request #{pr_number} in {owner}/{repo}.\n\
         1. Use `list_pr_files` to see which files changed.\n\
         2. Use `get_pr_diff` to read the unified diff.\n\
         3. Use `get_file_contents` when you need more context around a change.\n\
         4. Post a concise, actionable review comment using `post_pr_comment`.\n\
         Focus on correctness, security, and clarity. Be constructive."
    );

    let tools = pr_review_tools();

    match run_agent(agent, prompt, tools).await {
        Ok(text) => {
            info!(pr_number, "PR review completed");
            Some(Ok(text))
        }
        Err(e) => {
            warn!(pr_number, error = %e, "PR review agent failed");
            Some(Err(e.to_string()))
        }
    }
}

fn pr_review_tools() -> Vec<ToolDef> {
    vec![
        tool_def(
            "list_pr_files",
            "List the files changed in a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number"],
                "properties": {
                    "owner":     { "type": "string", "description": "Repository owner" },
                    "repo":      { "type": "string", "description": "Repository name" },
                    "pr_number": { "type": "integer", "description": "Pull request number" }
                }
            }),
        ),
        tool_def(
            "get_pr_diff",
            "Get the unified diff of a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number"],
                "properties": {
                    "owner":     { "type": "string" },
                    "repo":      { "type": "string" },
                    "pr_number": { "type": "integer" }
                }
            }),
        ),
        tool_def(
            "get_file_contents",
            "Read the contents of a file at a specific git ref.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "path"],
                "properties": {
                    "owner": { "type": "string" },
                    "repo":  { "type": "string" },
                    "path":  { "type": "string", "description": "File path in the repository" },
                    "ref":   { "type": "string", "description": "Git ref (branch, tag, SHA). Defaults to HEAD." }
                }
            }),
        ),
        tool_def(
            "post_pr_comment",
            "Post a comment on a pull request.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "pr_number", "body"],
                "properties": {
                    "owner":     { "type": "string" },
                    "repo":      { "type": "string" },
                    "pr_number": { "type": "integer" },
                    "body":      { "type": "string", "description": "Markdown comment body" }
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_review_tools_has_four_entries() {
        assert_eq!(pr_review_tools().len(), 4);
    }

    #[test]
    fn review_actions_contains_opened() {
        assert!(REVIEW_ACTIONS.contains(&"opened"));
        assert!(REVIEW_ACTIONS.contains(&"synchronize"));
        assert!(!REVIEW_ACTIONS.contains(&"closed"));
    }
}
