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
use super::{fetch_memory, run_agent};

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
         1. Use `get_file_contents` with path `.trogon/memory.md` — the result is a JSON object \
            with `sha` and `content`; note the `sha` (you will need it to update the file later).\n\
         2. Use `get_pr_comments` to recall any previous review discussion on this PR.\n\
         3. Use `list_pr_files` to see which files changed.\n\
         4. Use `get_pr_diff` to read the unified diff.\n\
         5. Use `get_file_contents` when you need more context around a specific change.\n\
         6. Post a concise, actionable review comment using `post_pr_comment`.\n\
         7. If you learned something important about the repo conventions, update `.trogon/memory.md` \
            using `update_file` (pass the `sha` from step 1, use a new branch), \
            then open a PR with `create_pull_request`.\n\
         Focus on correctness, security, and clarity. Be constructive."
    );

    let tools = pr_review_tools();

    // Pre-fetch memory file and inject as Anthropic system prompt.
    // Returns None gracefully when the file doesn't exist yet.
    let mem_path = agent.memory_path.as_deref().unwrap_or(super::DEFAULT_MEMORY_PATH);
    let memory = fetch_memory(agent, owner, repo, mem_path).await;

    match run_agent(agent, prompt, tools, memory).await {
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
            "Read the contents of a file at a specific git ref. Returns a JSON object with `sha` (blob SHA — required for update_file on existing files) and `content` (decoded UTF-8 text).",
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
            "get_pr_comments",
            "Get all comments on a pull request — use this to recall prior decisions and context.",
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
        tool_def(
            "update_file",
            "Create or update a file in the repository. Use this to update .trogon/memory.md with learned context.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "path", "message", "content"],
                "properties": {
                    "owner":   { "type": "string" },
                    "repo":    { "type": "string" },
                    "path":    { "type": "string", "description": "File path (e.g. .trogon/memory.md)" },
                    "message": { "type": "string", "description": "Commit message" },
                    "content": { "type": "string", "description": "Full file content (plain UTF-8)" },
                    "branch":  { "type": "string", "description": "Target branch. Defaults to main." },
                    "sha":     { "type": "string", "description": "Current blob SHA — required when updating an existing file." }
                }
            }),
        ),
        tool_def(
            "create_pull_request",
            "Open a pull request. Use this to propose changes such as memory file updates.",
            serde_json::json!({
                "type": "object",
                "required": ["owner", "repo", "title", "head"],
                "properties": {
                    "owner": { "type": "string" },
                    "repo":  { "type": "string" },
                    "title": { "type": "string", "description": "PR title" },
                    "head":  { "type": "string", "description": "Branch containing the changes" },
                    "base":  { "type": "string", "description": "Target branch. Defaults to main." },
                    "body":  { "type": "string", "description": "PR description (Markdown)" }
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_review_tools_has_seven_entries() {
        assert_eq!(pr_review_tools().len(), 7);
    }

    #[test]
    fn pr_review_tools_includes_memory_tools() {
        let tools = pr_review_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"get_pr_comments"));
        assert!(names.contains(&"update_file"));
        assert!(names.contains(&"create_pull_request"));
    }

    #[test]
    fn review_actions_contains_opened() {
        assert!(REVIEW_ACTIONS.contains(&"opened"));
        assert!(REVIEW_ACTIONS.contains(&"synchronize"));
        assert!(!REVIEW_ACTIONS.contains(&"closed"));
    }
}
