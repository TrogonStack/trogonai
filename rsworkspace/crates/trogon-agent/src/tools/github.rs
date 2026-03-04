//! GitHub API tools — all HTTP calls route through `trogon-secret-proxy`.
//!
//! URL pattern: `{proxy_url}/github/{github_api_path}`
//!
//! The proxy maps the `github` provider prefix to `https://api.github.com`,
//! so `{proxy_url}/github/repos/owner/repo/pulls/1` becomes
//! `https://api.github.com/repos/owner/repo/pulls/1` with the real token
//! resolved from Vault at request time.

use base64::{Engine as _, engine::general_purpose};
use serde_json::Value;

use super::ToolContext;

/// Get the unified diff of a pull request.
///
/// GitHub returns a plain-text diff when `Accept: application/vnd.github.diff`
/// is set.
pub async fn get_pr_diff(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}",
        ctx.proxy_url,
    );

    ctx.http_client
        .get(&url)
        .header(
            "Authorization",
            format!("Bearer {}", ctx.github_token),
        )
        .header("Accept", "application/vnd.github.diff")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
}

/// List the files changed in a pull request.
///
/// Returns a JSON array of file objects including `filename`, `status`, and
/// `patch` fields.
pub async fn list_pr_files(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}/files",
        ctx.proxy_url,
    );

    let files: Value = ctx
        .http_client
        .get(&url)
        .header(
            "Authorization",
            format!("Bearer {}", ctx.github_token),
        )
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    serde_json::to_string_pretty(&files).map_err(|e| e.to_string())
}

/// Read the UTF-8 contents of a file at a specific ref.
///
/// GitHub returns base64-encoded content in the `content` field; this
/// function decodes it before returning.
pub async fn get_file_contents(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let path = input["path"].as_str().ok_or("missing path")?;
    let git_ref = input["ref"].as_str().unwrap_or("HEAD");

    let url = format!(
        "{}/github/repos/{owner}/{repo}/contents/{path}?ref={git_ref}",
        ctx.proxy_url,
    );

    let response: Value = ctx
        .http_client
        .get(&url)
        .header(
            "Authorization",
            format!("Bearer {}", ctx.github_token),
        )
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    // GitHub embeds base64 content with embedded newlines — strip them first.
    let raw = response["content"]
        .as_str()
        .ok_or("missing content field in GitHub response")?
        .replace('\n', "");

    let bytes = general_purpose::STANDARD
        .decode(&raw)
        .map_err(|e| format!("base64 decode error: {e}"))?;

    String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {e}"))
}

/// Post a comment on a pull request (uses the Issues comments endpoint).
pub async fn post_pr_comment(ctx: &ToolContext, input: &Value) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;
    let body = input["body"].as_str().ok_or("missing body")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/issues/{pr_number}/comments",
        ctx.proxy_url,
    );

    let response: Value = ctx
        .http_client
        .post(&url)
        .header(
            "Authorization",
            format!("Bearer {}", ctx.github_token),
        )
        .header("Accept", "application/vnd.github.v3+json")
        .json(&serde_json::json!({ "body": body }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let url_str = response["html_url"].as_str().unwrap_or("(no url)");
    Ok(format!("Comment posted: {url_str}"))
}
