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
use tracing::warn;

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
/// ## Idempotency
///
/// GitHub does not honour the `Idempotency-Key` header for `POST /pulls`.
/// Instead, when `_idempotency_key` is present (recovery path), this function
/// first checks whether a PR from `head` → `base` already exists (in any state)
/// and returns it if found. This prevents creating a duplicate PR when the tool
/// result cache was not written before the process crashed — including the case
/// where the original PR was closed or merged between the crash and recovery
/// (which would otherwise bypass GitHub's own 422 duplicate guard).
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

    let auth_headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];

    // Build POST headers separately so the Idempotency-Key is always forwarded
    // to GitHub (even though GitHub currently ignores it for /pulls, forwarding
    // it keeps the behaviour consistent and future-proof).
    let mut post_headers = auth_headers.clone();
    if let Some(key) = idempotency_key {
        post_headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }

    // Recovery dedup: scan for any existing PR from `head` → `base` before
    // creating a new one. GitHub's own 422 guard only fires when the original
    // PR is still open; if it was closed or merged between the crash and
    // recovery, this check prevents creating a second PR.
    if idempotency_key.is_some() {
        let list_url = format!(
            "{}/github/repos/{owner}/{repo}/pulls?head={owner}:{head}&base={base}&state=all&per_page=5",
            ctx.proxy_url,
        );
        match ctx.http_client.get(&list_url, auth_headers).await {
            Ok(resp) => {
                if let Ok(prs) = serde_json::from_str::<Value>(&resp.body) {
                    if let Some(arr) = prs.as_array() {
                        if let Some(pr) = arr.first() {
                            let html_url = pr["html_url"].as_str().unwrap_or("(no url)");
                            let number = pr["number"].as_u64().unwrap_or(0);
                            return Ok(format!("Pull request #{number} opened: {html_url}"));
                        }
                    }
                }
            }
            // Graceful degradation: if the pre-check fails, proceed with the
            // create. GitHub's 422 guard still blocks obvious duplicates.
            Err(_) => {}
        }
    }

    let url = format!("{}/github/repos/{owner}/{repo}/pulls", ctx.proxy_url);

    let resp = ctx
        .http_client
        .post(
            &url,
            post_headers,
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
///
/// ## Idempotency
///
/// GitHub does not honour the `Idempotency-Key` header for reviewer requests.
/// When `_idempotency_key` is present (recovery path), this function first
/// fetches the current requested reviewers and skips the POST if all requested
/// reviewers are already in the list. This prevents sending duplicate review
/// request notifications to reviewers who were already requested before the
/// crash.
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

    let auth_headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];

    // Recovery dedup: filter out reviewers who are already in the pending list
    // before posting. The previous "all-or-nothing" check (skip only if ALL are
    // present) allowed duplicate notifications for reviewers added before a
    // crash when at least one reviewer was missing. Now we only request the
    // reviewers who are not yet present — those already added are silently
    // dropped so they don't receive a second notification.
    let reviewers_to_request: Vec<&str> = if idempotency_key.is_some() {
        match ctx.http_client.get(&url, auth_headers.clone()).await {
            Ok(resp) => {
                if let Ok(data) = serde_json::from_str::<Value>(&resp.body) {
                    let existing: Vec<&str> = data["users"]
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|u| u["login"].as_str()).collect())
                        .unwrap_or_default();
                    reviewers
                        .iter()
                        .copied()
                        .filter(|r| !existing.contains(r))
                        .collect()
                } else {
                    reviewers.clone()
                }
            }
            // Graceful degradation: if the pre-check fails, request all reviewers.
            Err(_) => reviewers.clone(),
        }
    } else {
        reviewers.clone()
    };

    if reviewers_to_request.is_empty() {
        return Ok(format!("Reviewers requested on PR #{pr_number}"));
    }

    let resp = ctx
        .http_client
        .post(
            &url,
            auth_headers,
            serde_json::json!({ "reviewers": reviewers_to_request }),
        )
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let number = response["number"].as_u64().unwrap_or(pr_number);
    Ok(format!("Reviewers requested on PR #{number}"))
}

/// Post a comment on a pull request (uses the Issues comments endpoint).
///
/// ## Idempotency
///
/// When `_idempotency_key` is present this function provides best-effort
/// duplicate suppression on recovery:
///
/// 1. Before posting, fetch existing comments on the PR and scan for an HTML
///    comment marker `<!-- trogon-idempotency-key: {key} -->` embedded in any
///    comment body. If found, return early without posting a second comment.
/// 2. When posting, append the marker to the body so future recovery passes can
///    detect the already-posted comment.
/// 3. If the pre-post fetch itself fails the function degrades gracefully and
///    proceeds with the POST (same behaviour as before).
///
/// Note: GitHub silently ignores the `Idempotency-Key` HTTP header for comment
/// endpoints, so we do not forward it.
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

    let auth_headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", ctx.github_token),
        ),
        (
            "Accept".to_string(),
            "application/vnd.github.v3+json".to_string(),
        ),
    ];

    // Dedup check: if an idempotency key is present, scan existing comments for
    // the embedded marker before posting.
    if let Some(key) = idempotency_key {
        let marker = super::idempotency_marker(key);

        // Paginate through all comments — the default API response is only ~30 items.
        // Without pagination, a comment on a PR with >30 comments may be on page 2+
        // and the dedup check misses it, causing a duplicate post.
        // Cap at 10 pages (1000 comments) to guard against unbounded loops.
        const DEDUP_PAGE_CAP: u32 = 10;
        'dedup: for page in 1u32..=DEDUP_PAGE_CAP {
            let page_url = format!("{url}?per_page=100&page={page}");
            match ctx.http_client.get(&page_url, auth_headers.clone()).await {
                Ok(resp) => {
                    match serde_json::from_str::<Value>(&resp.body) {
                        Ok(Value::Array(arr)) => {
                            for comment in &arr {
                                if comment["body"]
                                    .as_str()
                                    .map(|b| b.contains(&marker))
                                    .unwrap_or(false)
                                {
                                    let html_url =
                                        comment["html_url"].as_str().unwrap_or("(no url)");
                                    return Ok(format!("Comment posted: {html_url}"));
                                }
                            }
                            if arr.len() < 100 {
                                break 'dedup; // last page — marker not found
                            }
                            if page == DEDUP_PAGE_CAP {
                                // Reached the scan limit without finding the marker.
                                // A duplicate may be posted if the marker is beyond
                                // this page. This is extremely unlikely in practice.
                                warn!(
                                    pr = %format!("{owner}/{repo}#{pr_number}"),
                                    pages_scanned = DEDUP_PAGE_CAP,
                                    "post_pr_comment dedup scan reached page cap — \
                                    marker may exist beyond scanned range; duplicate comment possible"
                                );
                            }
                        }
                        _ => break 'dedup, // unexpected shape — stop paginating
                    }
                }
                // Graceful degradation: if we can't fetch comments, proceed with POST.
                Err(_) => break 'dedup,
            }
        }

        let effective_body = format!("{body}\n\n{marker}");
        let resp = ctx
            .http_client
            .post(&url, auth_headers, serde_json::json!({ "body": effective_body }))
            .await?;
        let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;
        let url_str = response["html_url"].as_str().unwrap_or("(no url)");
        return Ok(format!("Comment posted: {url_str}"));
    }

    let resp = ctx
        .http_client
        .post(&url, auth_headers, serde_json::json!({ "body": body }))
        .await?;
    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let url_str = response["html_url"].as_str().unwrap_or("(no url)");
    Ok(format!("Comment posted: {url_str}"))
}
