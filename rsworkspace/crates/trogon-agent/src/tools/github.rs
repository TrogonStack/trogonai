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

use super::{HttpClient, ToolContext};

/// Get the unified diff of a pull request.
///
/// GitHub returns a plain-text diff when `Accept: application/vnd.github.diff`
/// is set.
pub async fn get_pr_diff(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}",
        ctx.proxy_url,
    );

    let resp = ctx
        .http_client
        .get(
            &url,
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.github_token),
                ),
                (
                    "Accept".to_string(),
                    "application/vnd.github.diff".to_string(),
                ),
            ],
        )
        .await?;
    Ok(resp.body)
}

/// List the files changed in a pull request.
///
/// Returns a JSON array of file objects including `filename`, `status`, and
/// `patch` fields.
pub async fn list_pr_files(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}/files",
        ctx.proxy_url,
    );

    let resp = ctx
        .http_client
        .get(
            &url,
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.github_token),
                ),
                (
                    "Accept".to_string(),
                    "application/vnd.github.v3+json".to_string(),
                ),
            ],
        )
        .await?;
    let files: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&files).map_err(|e| e.to_string())
}

/// Read the UTF-8 contents of a file at a specific ref.
///
/// GitHub returns base64-encoded content in the `content` field; this
/// function decodes it and returns a JSON object with two fields:
///
/// - `"sha"` — the blob SHA (store this; required by `update_file` when the file already exists)
/// - `"content"` — the decoded UTF-8 text
pub async fn get_file_contents(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let path = input["path"].as_str().ok_or("missing path")?;
    let git_ref = input["ref"].as_str().unwrap_or("HEAD");

    let url = format!(
        "{}/github/repos/{owner}/{repo}/contents/{path}?ref={git_ref}",
        ctx.proxy_url,
    );

    let resp = ctx
        .http_client
        .get(
            &url,
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.github_token),
                ),
                (
                    "Accept".to_string(),
                    "application/vnd.github.v3+json".to_string(),
                ),
            ],
        )
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    // GitHub embeds base64 content with embedded newlines — strip them first.
    let raw = response["content"]
        .as_str()
        .ok_or("missing content field in GitHub response")?
        .replace('\n', "");

    let bytes = general_purpose::STANDARD
        .decode(&raw)
        .map_err(|e| format!("base64 decode error: {e}"))?;

    let content = String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {e}"))?;
    let sha = response["sha"].as_str().unwrap_or("").to_string();

    serde_json::to_string(&serde_json::json!({ "sha": sha, "content": content }))
        .map_err(|e| e.to_string())
}

/// Get all comments on a pull request (uses the Issues comments endpoint).
///
/// Returns a JSON array of comments — suitable for injecting as prior-conversation memory.
pub async fn get_pr_comments(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;

    let url = format!(
        "{}/github/repos/{owner}/{repo}/issues/{pr_number}/comments",
        ctx.proxy_url,
    );

    let resp = ctx
        .http_client
        .get(
            &url,
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", ctx.github_token),
                ),
                (
                    "Accept".to_string(),
                    "application/vnd.github.v3+json".to_string(),
                ),
            ],
        )
        .await?;
    let comments: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&comments).map_err(|e| e.to_string())
}

/// Create or update a file in a repository.
///
/// If the file already exists, `sha` (the current blob SHA) must be provided.
/// The `content` field must be plain UTF-8 text — this function base64-encodes it.
pub async fn update_file(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let path = input["path"].as_str().ok_or("missing path")?;
    let message = input["message"].as_str().ok_or("missing message")?;
    let content = input["content"].as_str().ok_or("missing content")?;
    let branch = input["branch"].as_str().unwrap_or("main");
    let idempotency_key = input["_idempotency_key"].as_str();

    let encoded = general_purpose::STANDARD.encode(content.as_bytes());

    let mut body = serde_json::json!({
        "message": message,
        "content": encoded,
        "branch": branch,
    });

    if let Some(sha) = input["sha"].as_str() {
        body["sha"] = serde_json::json!(sha);
    }

    let url = format!(
        "{}/github/repos/{owner}/{repo}/contents/{path}",
        ctx.proxy_url,
    );

    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }

    let resp = ctx.http_client.put(&url, headers, body).await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let commit_sha = response["commit"]["sha"].as_str().unwrap_or("(no sha)");
    Ok(format!("File updated: {path} — commit {commit_sha}"))
}

/// Open a pull request.
///
/// ## Idempotency limitation
///
/// GitHub does not honour the `Idempotency-Key` header — it is forwarded but
/// silently ignored. If the tool result was not cached and the process crashes
/// after this call, recovery will attempt a second `POST /pulls`. GitHub will
/// reject the duplicate with a 422 ("A pull request already exists") error if a
/// PR for the same `head` → `base` pair is still open, so a duplicate PR is
/// unlikely in practice. However, if the original PR was closed or merged
/// between the crash and recovery, a second PR **will** be created.
pub async fn create_pull_request(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let title = input["title"].as_str().ok_or("missing title")?;
    let head = input["head"].as_str().ok_or("missing head")?;
    let base = input["base"].as_str().unwrap_or("main");
    let body = input["body"].as_str().unwrap_or("");
    let idempotency_key = input["_idempotency_key"].as_str();

    let url = format!("{}/github/repos/{owner}/{repo}/pulls", ctx.proxy_url);

    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }

    let resp = ctx
        .http_client
        .post(
            &url,
            headers,
            serde_json::json!({
                "title": title,
                "head": head,
                "base": base,
                "body": body,
            }),
        )
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let html_url = response["html_url"].as_str().unwrap_or("(no url)");
    let number = response["number"].as_u64().unwrap_or(0);
    Ok(format!("Pull request #{number} opened: {html_url}"))
}

/// Request reviewers on a pull request.
pub async fn request_reviewers(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;
    let idempotency_key = input["_idempotency_key"].as_str();

    let reviewers: Vec<&str> = input["reviewers"]
        .as_array()
        .ok_or("missing reviewers array")?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}/requested_reviewers",
        ctx.proxy_url,
    );

    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }

    let resp = ctx
        .http_client
        .post(&url, headers, serde_json::json!({ "reviewers": reviewers }))
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let number = response["number"].as_u64().unwrap_or(pr_number);
    Ok(format!("Reviewers requested on PR #{number}"))
}

/// Post a comment on a pull request (uses the Issues comments endpoint).
///
/// ## Idempotency limitation
///
/// GitHub does not honour the `Idempotency-Key` header. Unlike
/// `create_pull_request`, GitHub has no duplicate-rejection mechanism for
/// issue/PR comments — a second POST will always create a second comment.
/// Protection relies entirely on the durable promise tool-result cache: if the
/// cache write succeeded before the crash, recovery replays the cached result
/// and never calls this function again. If the cache write also failed, a
/// duplicate comment will appear on recovery.
pub async fn post_pr_comment(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;
    let body = input["body"].as_str().ok_or("missing body")?;
    let idempotency_key = input["_idempotency_key"].as_str();

    let url = format!(
        "{}/github/repos/{owner}/{repo}/issues/{pr_number}/comments",
        ctx.proxy_url,
    );

    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }

    let resp = ctx
        .http_client
        .post(&url, headers, serde_json::json!({ "body": body }))
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let url_str = response["html_url"].as_str().unwrap_or("(no url)");
    Ok(format!("Comment posted: {url_str}"))
}
