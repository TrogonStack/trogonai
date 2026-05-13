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

/// Prefix each line of a unified diff patch with its 1-based position number.
///
/// GitHub's pull-review API requires `position` — a 1-based index into the
/// raw diff hunk — not a source-file line number. By annotating the patch
/// before handing it to the model the LLM can reference position numbers
/// directly without arithmetic.
pub fn annotate_diff(patch: &str) -> String {
    patch
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{} {line}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// List the files changed in a pull request.
///
/// Returns a JSON array of file objects including `filename`, `status`, and
/// `patch` fields. Each `patch` has its lines prefixed with a 1-based position
/// number so the model can reference them directly when calling `post_pr_review`.
/// Files without a `patch` field (binary or too large) are left as-is.
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
    let mut files: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;

    let arr = files
        .as_array_mut()
        .ok_or_else(|| format!("unexpected response from GitHub: {}", resp.body))?;

    for file in arr.iter_mut() {
        if let Some(patch) = file["patch"].as_str() {
            let annotated = annotate_diff(patch);
            file["patch"] = serde_json::json!(annotated);
        }
    }

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
        if let Ok(resp) = ctx.http_client.get(&list_url, auth_headers).await
            && let Ok(prs) = serde_json::from_str::<Value>(&resp.body)
            && let Some(arr) = prs.as_array()
            && let Some(pr) = arr.first()
        {
            let html_url = pr["html_url"].as_str().unwrap_or("(no url)");
            let number = pr["number"].as_u64().unwrap_or(0);
            return Ok(format!("Pull request #{number} opened: {html_url}"));
        }
        // Graceful degradation: if the pre-check fails, proceed with the
        // create. GitHub's 422 guard still blocks obvious duplicates.
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
            .post(
                &url,
                auth_headers,
                serde_json::json!({ "body": effective_body }),
            )
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

/// Post a pull request review with optional inline diff comments.
///
/// `comments` is a JSON array of `{"path", "position", "body"}` objects where
/// `position` is the 1-based line index in the file's annotated diff patch
/// (as returned by `list_pr_files`).  GitHub requires `commit_id` when
/// `comments` is non-empty; pass the PR head SHA as `commit_sha`.
pub async fn post_pr_review(
    ctx: &ToolContext<impl HttpClient>,
    input: &Value,
) -> Result<String, String> {
    let owner = input["owner"].as_str().ok_or("missing owner")?;
    let repo = input["repo"].as_str().ok_or("missing repo")?;
    let pr_number = input["pr_number"].as_u64().ok_or("missing pr_number")?;
    let body = input["body"].as_str().unwrap_or("");
    let event = input["event"].as_str().unwrap_or("COMMENT");
    let commit_sha = input["commit_sha"].as_str().unwrap_or("");

    let url = format!(
        "{}/github/repos/{owner}/{repo}/pulls/{pr_number}/reviews",
        ctx.proxy_url,
    );

    let mut payload = serde_json::json!({
        "body": body,
        "event": event,
    });

    if !commit_sha.is_empty() {
        payload["commit_id"] = serde_json::json!(commit_sha);
    }

    if let Some(comments) = input["comments"].as_array() {
        if !comments.is_empty() {
            payload["comments"] = serde_json::json!(comments);
        }
    }

    let resp = ctx
        .http_client
        .post(
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
            payload,
        )
        .await?;

    let response: Value = serde_json::from_str(&resp.body).map_err(|e| e.to_string())?;
    let review_id = response["id"].as_u64().unwrap_or(0);
    let html_url = response["html_url"].as_str().unwrap_or("(no url)");
    Ok(format!("Review posted: #{review_id} — {html_url}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use serde_json::json;

    fn make_ctx() -> crate::tools::ToolContext<crate::tools::mock::MockHttpClient> {
        crate::tools::ToolContext::for_test("http://proxy.test", "tok_github", "", "")
    }

    // ── get_pr_diff ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_pr_diff_returns_diff_body() {
        let ctx = make_ctx();
        ctx.http_client
            .enqueue_ok(200, "diff --git a/foo.rs b/foo.rs\n+new line\n");
        let result = get_pr_diff(
            &ctx,
            &json!({"owner": "owner", "repo": "repo", "pr_number": 1}),
        )
        .await;
        assert!(result.is_ok(), "get_pr_diff must succeed: {result:?}");
        assert!(result.unwrap().contains("diff --git"));
    }

    #[tokio::test]
    async fn get_pr_diff_http_error_propagates() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_err("connection refused");
        let result = get_pr_diff(
            &ctx,
            &json!({"owner": "owner", "repo": "repo", "pr_number": 1}),
        )
        .await;
        assert!(result.is_err(), "HTTP error must propagate as Err");
    }

    // ── list_pr_files ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_pr_files_returns_pretty_json() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!([{"filename": "src/main.rs", "status": "modified", "patch": "@@ -1 +1 @@\n"}])
                .to_string(),
        );
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 5})).await;
        assert!(result.is_ok(), "list_pr_files must succeed: {result:?}");
        let body = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed[0]["filename"], "src/main.rs");
    }

    #[tokio::test]
    async fn list_pr_files_json_parse_error_propagates() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(200, "not-valid-json{{");
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 5})).await;
        assert!(result.is_err(), "invalid JSON must propagate as Err");
    }

    #[tokio::test]
    async fn list_pr_files_non_array_json_returns_err() {
        // GitHub error responses are JSON objects, not arrays.
        // The function must return Err rather than forwarding the object to the LLM.
        let ctx = make_ctx();
        ctx.http_client
            .enqueue_ok(200, json!({"message": "Not Found"}).to_string());
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 9})).await;
        assert!(result.is_err(), "non-array JSON must return Err: {result:?}");
    }

    // ── get_file_contents ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_file_contents_decodes_base64_utf8() {
        let ctx = make_ctx();
        let text = "fn main() {}\n";
        let encoded = general_purpose::STANDARD.encode(text.as_bytes());
        ctx.http_client.enqueue_ok(
            200,
            json!({"sha": "abc123", "content": encoded}).to_string(),
        );
        let result = get_file_contents(
            &ctx,
            &json!({"owner": "o", "repo": "r", "path": "src/main.rs"}),
        )
        .await;
        assert!(result.is_ok(), "get_file_contents must succeed: {result:?}");
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["content"], text);
        assert_eq!(v["sha"], "abc123");
    }

    #[tokio::test]
    async fn get_file_contents_missing_content_field_returns_err() {
        let ctx = make_ctx();
        // Response has no `content` field.
        ctx.http_client
            .enqueue_ok(200, json!({"sha": "abc123"}).to_string());
        let result = get_file_contents(
            &ctx,
            &json!({"owner": "o", "repo": "r", "path": "README.md"}),
        )
        .await;
        assert!(result.is_err(), "missing content field must return Err");
        assert!(
            result.unwrap_err().contains("missing content field"),
            "error must describe the missing field"
        );
    }

    #[tokio::test]
    async fn get_file_contents_invalid_base64_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"sha": "x", "content": "!!!not-base64!!!"}).to_string(),
        );
        let result =
            get_file_contents(&ctx, &json!({"owner": "o", "repo": "r", "path": "f.bin"})).await;
        assert!(result.is_err(), "invalid base64 must return Err");
        assert!(
            result.unwrap_err().contains("base64 decode error"),
            "error must mention base64 decode"
        );
    }

    #[tokio::test]
    async fn get_file_contents_github_newlines_in_content_are_stripped() {
        let ctx = make_ctx();
        // GitHub embeds \n in the base64 string every 60 chars — strip before decoding.
        let text = "hello";
        let encoded_with_newlines =
            format!("{}\n", general_purpose::STANDARD.encode(text.as_bytes()));
        ctx.http_client.enqueue_ok(
            200,
            json!({"sha": "s1", "content": encoded_with_newlines}).to_string(),
        );
        let result = get_file_contents(
            &ctx,
            &json!({"owner": "o", "repo": "r", "path": "hello.txt"}),
        )
        .await;
        assert!(
            result.is_ok(),
            "newlines in base64 must be stripped: {result:?}"
        );
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["content"], text);
    }

    /// Valid base64 that decodes to non-UTF-8 bytes must return
    /// `Err("UTF-8 decode error: ...")` — distinct from the invalid-base64 path.
    #[tokio::test]
    async fn get_file_contents_non_utf8_bytes_returns_err() {
        let ctx = make_ctx();
        // 0xFF 0xFE is valid base64 when encoded, but not valid UTF-8.
        let non_utf8_b64 = general_purpose::STANDARD.encode([0xFF, 0xFE]);
        ctx.http_client.enqueue_ok(
            200,
            json!({"sha": "x", "content": non_utf8_b64}).to_string(),
        );
        let result = get_file_contents(
            &ctx,
            &json!({"owner": "o", "repo": "r", "path": "binary.bin"}),
        )
        .await;
        assert!(
            result.is_err(),
            "non-UTF-8 bytes must return Err: {result:?}"
        );
        assert!(
            result.unwrap_err().contains("UTF-8 decode error"),
            "error must mention 'UTF-8 decode error'"
        );
    }

    // ── get_pr_comments ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_pr_comments_returns_pretty_json() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!([{"id": 1, "body": "lgtm", "html_url": "https://github.com/o/r/issues/1#comment-1"}])
                .to_string(),
        );
        let result =
            get_pr_comments(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 1})).await;
        assert!(result.is_ok(), "get_pr_comments must succeed: {result:?}");
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed[0]["body"], "lgtm");
    }

    // ── update_file ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn update_file_without_sha_creates_file() {
        let ctx = make_ctx();
        ctx.http_client
            .enqueue_ok(201, json!({"commit": {"sha": "commit-abc"}}).to_string());
        let result = update_file(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "path": "new.txt",
                "message": "add file", "content": "hello"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "update_file (create) must succeed: {result:?}"
        );
        assert!(result.unwrap().contains("commit-abc"));
    }

    #[tokio::test]
    async fn update_file_with_sha_updates_existing_file() {
        let ctx = make_ctx();
        ctx.http_client
            .enqueue_ok(200, json!({"commit": {"sha": "commit-def"}}).to_string());
        let result = update_file(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "path": "existing.txt",
                "message": "update file", "content": "new content",
                "sha": "old-blob-sha"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "update_file (update) must succeed: {result:?}"
        );
        assert!(result.unwrap().contains("commit-def"));
    }

    #[tokio::test]
    async fn update_file_with_idempotency_key_succeeds() {
        let ctx = make_ctx();
        ctx.http_client
            .enqueue_ok(200, json!({"commit": {"sha": "commit-idem"}}).to_string());
        let result = update_file(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "path": "f.txt",
                "message": "update", "content": "x",
                "_idempotency_key": "idem-key-1"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "update_file with idempotency key must succeed: {result:?}"
        );
    }

    // ── create_pull_request — pre-check returns empty array ───────────────────

    /// When the pre-check GET succeeds but returns an empty array (no existing PR),
    /// the function must proceed to POST and create a new PR.
    #[tokio::test]
    async fn create_pull_request_precheck_empty_array_proceeds_to_post() {
        let ctx = make_ctx();

        // Pre-check: array is empty — no existing PR found.
        ctx.http_client.enqueue_ok(200, json!([]).to_string());

        // POST: new PR created.
        ctx.http_client.enqueue_ok(
            201,
            json!({"number": 10, "html_url": "https://github.com/o/r/pull/10"}).to_string(),
        );

        let result = create_pull_request(
            &ctx,
            &json!({
                "owner": "o", "repo": "r",
                "title": "feat", "head": "feat/branch",
                "_idempotency_key": "pr-key-2"
            }),
        )
        .await;
        assert!(result.is_ok(), "must succeed: {result:?}");
        assert!(
            result.unwrap().contains("Pull request #10"),
            "must report the newly created PR number"
        );
    }

    // ── request_reviewers — no idempotency key ────────────────────────────────

    /// First-time execution (no `_idempotency_key`) skips the GET pre-check
    /// and posts all reviewers directly.
    #[tokio::test]
    async fn request_reviewers_no_idempotency_key_posts_all_reviewers() {
        let ctx = make_ctx();
        // No GET enqueued — the pre-check must be skipped.
        ctx.http_client
            .enqueue_ok(201, json!({"number": 3}).to_string());
        let result = request_reviewers(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 3,
                "reviewers": ["alice", "bob"]
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "request_reviewers (first-time) must succeed: {result:?}"
        );
        assert!(result.unwrap().contains("Reviewers requested"));
    }

    // ── post_pr_comment — no idempotency key ─────────────────────────────────

    /// First-time execution (no `_idempotency_key`) skips dedup scanning
    /// and posts directly.
    #[tokio::test]
    async fn post_pr_comment_no_idempotency_key_posts_directly() {
        let ctx = make_ctx();
        // No pagination GETs enqueued — dedup scan must be skipped entirely.
        ctx.http_client.enqueue_ok(
            201,
            json!({"id": 777, "html_url": "https://github.com/o/r/issues/1#issuecomment-777"})
                .to_string(),
        );
        let result = post_pr_comment(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "body": "LGTM"}),
        )
        .await;
        assert!(result.is_ok(), "first-time post must succeed: {result:?}");
        assert!(result.unwrap().contains("Comment posted"));
    }

    /// When the dedup scan reaches `DEDUP_PAGE_CAP` (10 pages × 100 comments)
    /// without finding the idempotency marker, the function emits a warning and
    /// still posts the comment — verifying it does not hang or skip the POST.
    #[tokio::test]
    async fn post_pr_comment_dedup_page_cap_reached_still_posts() {
        let ctx = make_ctx();

        // Build a page of 100 comments, none containing the idempotency marker.
        let page_items: Vec<serde_json::Value> = (0u32..100)
            .map(|i| {
                json!({
                    "id": i,
                    "body": format!("comment body {i} — no marker here"),
                    "html_url": format!("https://github.com/owner/repo/issues/1#comment-{i}")
                })
            })
            .collect();
        let page_json = serde_json::to_string(&page_items).unwrap();

        // Enqueue 10 full pages (DEDUP_PAGE_CAP = 10).
        for _ in 0..10 {
            ctx.http_client.enqueue_ok(200, page_json.clone());
        }

        // Final POST — the actual comment creation.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "id": 99999,
                "html_url": "https://github.com/owner/repo/issues/1#issuecomment-99999"
            })
            .to_string(),
        );

        let input = json!({
            "owner": "owner",
            "repo": "repo",
            "pr_number": 1,
            "body": "New review comment",
            "_idempotency_key": "dedup-cap-test-key"
        });

        let result = post_pr_comment(&ctx, &input).await;
        assert!(
            result.is_ok(),
            "post_pr_comment must succeed even after reaching dedup page cap: {result:?}"
        );
        assert!(
            result.unwrap().contains("Comment posted"),
            "must return a 'Comment posted' message after the page-cap scan"
        );
    }

    /// When the pre-check GET on existing PRs fails, `create_pull_request`
    /// degrades gracefully and proceeds with the POST instead of returning an
    /// error.
    #[tokio::test]
    async fn create_pull_request_precheck_get_fails_proceeds_with_post() {
        let ctx = make_ctx();

        // Pre-check GET fails.
        ctx.http_client.enqueue_err("connection refused");

        // POST succeeds — the PR is created.
        ctx.http_client.enqueue_ok(
            201,
            json!({
                "number": 42,
                "html_url": "https://github.com/owner/repo/pull/42"
            })
            .to_string(),
        );

        let input = json!({
            "owner": "owner",
            "repo": "repo",
            "title": "My PR",
            "head": "feat/my-branch",
            "base": "main",
            "_idempotency_key": "pr-create-key"
        });

        let result = create_pull_request(&ctx, &input).await;
        assert!(
            result.is_ok(),
            "create_pull_request must succeed after GET failure: {result:?}"
        );
        assert!(
            result.unwrap().contains("Pull request #42"),
            "must contain the newly created PR number"
        );
    }

    /// When `base` is absent from the input, `create_pull_request` defaults to
    /// `"main"` (line 254: `unwrap_or("main")`).  The function must still
    /// succeed and return the newly created PR number.
    #[tokio::test]
    async fn create_pull_request_base_absent_defaults_to_main() {
        let ctx = make_ctx();

        // No `_idempotency_key` → pre-check GET is skipped; only the POST runs.
        ctx.http_client.enqueue_ok(
            201,
            json!({
                "number": 77,
                "html_url": "https://github.com/o/r/pull/77"
            })
            .to_string(),
        );

        let result = create_pull_request(
            &ctx,
            &json!({
                "owner": "o", "repo": "r",
                "title": "feat: add thing",
                "head": "feat/add-thing"
                // "base" intentionally absent — should default to "main"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "create_pull_request must succeed when base is absent: {result:?}"
        );
        assert!(
            result.unwrap().contains("Pull request #77"),
            "must report the newly created PR number"
        );
        assert!(
            ctx.http_client.is_empty(),
            "only one HTTP call (POST) should have been made"
        );
    }

    /// When only some of the requested reviewers are already in the pending list,
    /// `request_reviewers` must POST only the new ones — not the full original list.
    #[tokio::test]
    async fn request_reviewers_partial_dedup_posts_only_new_reviewers() {
        let ctx = make_ctx();

        // GET: alice is already pending, bob and carol are not.
        ctx.http_client
            .enqueue_ok(200, json!({ "users": [{ "login": "alice" }] }).to_string());
        // POST: bob and carol are requested.
        ctx.http_client
            .enqueue_ok(201, json!({ "number": 5 }).to_string());

        let result = request_reviewers(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 5,
                "reviewers": ["alice", "bob", "carol"],
                "_idempotency_key": "partial-dedup-key"
            }),
        )
        .await;

        assert!(result.is_ok(), "partial dedup must succeed: {result:?}");
        assert!(result.unwrap().contains("Reviewers requested on PR #5"));
        assert!(
            ctx.http_client.is_empty(),
            "both GET and POST must have been consumed"
        );
    }

    /// When the idempotency marker is found in an existing comment body,
    /// `post_pr_comment` must return early — skipping the POST entirely.
    #[tokio::test]
    async fn post_pr_comment_marker_found_in_existing_comment_skips_post() {
        let ctx = make_ctx();

        let key = "idem-key-pr-1";
        let marker = crate::tools::idempotency_marker(key);

        // One page with a comment that already contains the marker.
        ctx.http_client.enqueue_ok(
            200,
            json!([
                {
                    "id": 111,
                    "body": format!("Prior triage note.\n\n{marker}"),
                    "html_url": "https://github.com/o/r/issues/1#issuecomment-111"
                }
            ])
            .to_string(),
        );
        // No POST enqueued — the function must return before reaching the POST.

        let result = post_pr_comment(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 1,
                "body": "Triage comment",
                "_idempotency_key": key
            }),
        )
        .await;

        assert!(result.is_ok(), "must succeed on dedup hit: {result:?}");
        assert!(
            result.unwrap().contains("Comment posted"),
            "must return the existing comment URL"
        );
        assert!(
            ctx.http_client.is_empty(),
            "no POST should have been made — marker was found in existing comment"
        );
    }

    /// When `branch` is absent from the input, `update_file` defaults to `"main"`
    /// (line 194: `unwrap_or("main")`). The function must still succeed.
    #[tokio::test]
    async fn update_file_branch_absent_defaults_to_main() {
        let ctx = make_ctx();

        ctx.http_client.enqueue_ok(
            200,
            json!({
                "content": {"name": "README.md"},
                "commit": {"sha": "abc123def456"}
            })
            .to_string(),
        );

        let result = update_file(
            &ctx,
            &json!({
                "owner": "o", "repo": "r",
                "path": "README.md",
                "message": "docs: update readme",
                "content": "# Hello\n"
                // "branch" intentionally absent — should default to "main"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "update_file must succeed when branch is absent: {result:?}"
        );
        assert!(
            result.unwrap().contains("README.md"),
            "result must mention the file path"
        );
        assert!(
            ctx.http_client.is_empty(),
            "exactly one PUT must have been made"
        );
    }

    /// When the HTTP response body is not valid JSON, `get_pr_comments` must
    /// return an Err rather than panicking or returning garbage.
    #[tokio::test]
    async fn get_pr_comments_json_parse_error_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(200, "not valid json {{");

        let result =
            get_pr_comments(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 1})).await;

        assert!(
            result.is_err(),
            "invalid JSON response must return Err: {result:?}"
        );
    }

    /// When the pre-check GET succeeds with HTTP 200 but the body is not valid
    /// JSON, `request_reviewers` must degrade gracefully and request all
    /// reviewers (line 387-388: `else { reviewers.clone() }`).
    #[tokio::test]
    async fn request_reviewers_get_invalid_json_falls_back_to_all_reviewers() {
        let ctx = make_ctx();

        // GET returns 200 OK but body is malformed — JSON parse will fail.
        ctx.http_client.enqueue_ok(200, "not valid json {{");

        // POST with all reviewers must follow.
        ctx.http_client
            .enqueue_ok(201, json!({"number": 9}).to_string());

        let result = request_reviewers(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 9,
                "reviewers": ["alice", "bob"],
                "_idempotency_key": "invalid-json-key"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "must succeed after invalid JSON fallback: {result:?}"
        );
        assert!(result.unwrap().contains("Reviewers requested on PR #9"));
        assert!(
            ctx.http_client.is_empty(),
            "both GET and POST must have been consumed"
        );
    }

    /// When all requested reviewers are already in the PR's pending reviewer
    /// list, `request_reviewers` must skip the POST entirely and return a
    /// success message without making the second HTTP call.
    #[tokio::test]
    async fn request_reviewers_all_already_assigned_skips_post() {
        let ctx = make_ctx();

        // GET existing reviewers — both alice and bob are already pending.
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "users": [
                    { "login": "alice" },
                    { "login": "bob" }
                ]
            })
            .to_string(),
        );
        // No POST enqueued — the function must not make a second HTTP call.

        let input = json!({
            "owner": "owner",
            "repo": "repo",
            "pr_number": 7,
            "reviewers": ["alice", "bob"],
            "_idempotency_key": "reviewers-dedup-key"
        });

        let result = request_reviewers(&ctx, &input).await;
        assert!(
            result.is_ok(),
            "request_reviewers must succeed when all reviewers are already assigned: {result:?}"
        );
        assert!(
            result.unwrap().contains("Reviewers requested"),
            "must return success message"
        );
        // Verify no POST was made by checking the mock queue is now empty.
        // (Any unexpected call would panic with "queue empty" inside enqueue_ok).
        assert!(
            ctx.http_client.is_empty(),
            "no POST should have been made when all reviewers are already assigned"
        );
    }

    // ── annotate_diff ─────────────────────────────────────────────────────────

    #[test]
    fn annotate_diff_numbers_lines_from_one() {
        let patch = "@@ -1,3 +1,4 @@\n fn foo() {}\n+fn bar() {}\n fn baz() {}";
        let annotated = annotate_diff(patch);
        let lines: Vec<&str> = annotated.lines().collect();
        assert_eq!(lines[0], "1 @@ -1,3 +1,4 @@");
        assert_eq!(lines[1], "2  fn foo() {}");
        assert_eq!(lines[2], "3 +fn bar() {}");
        assert_eq!(lines[3], "4  fn baz() {}");
    }

    #[test]
    fn annotate_diff_empty_patch_returns_empty() {
        assert_eq!(annotate_diff(""), "");
    }

    // ── list_pr_files annotates patches ──────────────────────────────────────

    #[tokio::test]
    async fn list_pr_files_annotates_patch_lines() {
        let ctx = make_ctx();
        let patch = "@@ -1,2 +1,3 @@\n line1\n+line2";
        ctx.http_client.enqueue_ok(
            200,
            json!([{
                "filename": "src/main.rs",
                "status": "modified",
                "patch": patch
            }])
            .to_string(),
        );
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 1})).await;
        assert!(result.is_ok(), "list_pr_files must succeed: {result:?}");
        let parsed: Value = serde_json::from_str(&result.unwrap()).unwrap();
        let annotated_patch = parsed[0]["patch"].as_str().unwrap();
        assert!(
            annotated_patch.starts_with("1 "),
            "patch must start with position 1: {annotated_patch}"
        );
        assert!(
            annotated_patch.contains("2  line1"),
            "second line must be prefixed with 2: {annotated_patch}"
        );
    }

    #[tokio::test]
    async fn list_pr_files_leaves_files_without_patch_unchanged() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!([{
                "filename": "image.png",
                "status": "added"
                // no "patch" field — binary file
            }])
            .to_string(),
        );
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 2})).await;
        assert!(result.is_ok(), "must succeed for binary file: {result:?}");
        let parsed: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(
            parsed[0]["patch"].is_null(),
            "binary file must have no patch field"
        );
    }

    // ── post_pr_review ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn post_pr_review_returns_review_id_and_url() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({
                "id": 42,
                "html_url": "https://github.com/o/r/pull/1#pullrequestreview-42"
            })
            .to_string(),
        );
        let result = post_pr_review(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 1,
                "commit_sha": "abc123",
                "body": "Looks good overall.",
                "event": "COMMENT",
                "comments": [{"path": "src/lib.rs", "position": 3, "body": "Risky call"}]
            }),
        )
        .await;
        assert!(result.is_ok(), "post_pr_review must succeed: {result:?}");
        let msg = result.unwrap();
        assert!(msg.contains("#42"), "must include review id: {msg}");
        assert!(
            msg.contains("pullrequestreview-42"),
            "must include html_url: {msg}"
        );
    }

    #[tokio::test]
    async fn post_pr_review_without_comments_omits_comments_field() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"id": 7, "html_url": "https://github.com/o/r/pull/2#pullrequestreview-7"})
                .to_string(),
        );
        let result = post_pr_review(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 2,
                "body": "No issues found.",
                "event": "APPROVE"
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "post_pr_review without comments must succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn post_pr_review_http_error_propagates() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_err("connection refused");
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_err(), "HTTP error must propagate: {result:?}");
    }

    #[tokio::test]
    async fn post_pr_review_missing_owner_returns_err() {
        let ctx = make_ctx();
        let result = post_pr_review(
            &ctx,
            &json!({"repo": "r", "pr_number": 1, "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_err(), "missing owner must return Err: {result:?}");
        assert!(
            result.unwrap_err().contains("missing owner"),
            "error must name the missing field"
        );
    }

    #[tokio::test]
    async fn post_pr_review_non_json_response_returns_err() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(200, "this is not json {{");
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_err(), "non-JSON response must return Err: {result:?}");
    }

    #[tokio::test]
    async fn post_pr_review_response_missing_id_and_url_uses_fallback() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(200, json!({}).to_string());
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_ok(), "empty-body response must succeed: {result:?}");
        let msg = result.unwrap();
        assert!(msg.contains("#0"), "fallback id must be 0: {msg}");
        assert!(msg.contains("(no url)"), "fallback url must appear: {msg}");
    }

    #[tokio::test]
    async fn post_pr_review_empty_comments_array_succeeds() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"id": 5, "html_url": "https://github.com/o/r/pull/1#pullrequestreview-5"})
                .to_string(),
        );
        let result = post_pr_review(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 1,
                "body": "LGTM", "event": "APPROVE",
                "comments": []
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "empty comments array must succeed: {result:?}"
        );
    }

    // ── annotate_diff edge cases ──────────────────────────────────────────────

    #[test]
    fn annotate_diff_multi_hunk_positions_are_continuous() {
        // Positions must count continuously across hunk boundaries —
        // the first line of the second hunk is NOT reset to 1.
        let patch = "@@ -1,2 +1,2 @@\n context\n-old\n+new\n@@ -10,2 +10,2 @@\n other\n-foo\n+bar";
        let annotated = annotate_diff(patch);
        let lines: Vec<&str> = annotated.lines().collect();
        assert_eq!(lines[0], "1 @@ -1,2 +1,2 @@");
        assert_eq!(lines[3], "4 +new");
        // Second hunk header continues from position 5, not 1.
        assert_eq!(lines[4], "5 @@ -10,2 +10,2 @@");
        assert_eq!(lines[7], "8 +bar");
    }

    #[test]
    fn annotate_diff_trailing_newline_does_not_produce_extra_line() {
        let patch = "@@ -1 +1 @@\n+line\n";
        let annotated = annotate_diff(patch);
        // `str::lines()` strips the trailing newline — result must have 2 lines.
        assert_eq!(annotated.lines().count(), 2);
        assert_eq!(annotated.lines().last().unwrap(), "2 +line");
    }

    // ── list_pr_files edge cases ──────────────────────────────────────────────

    #[tokio::test]
    async fn list_pr_files_multiple_files_annotates_each_patch_independently() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!([
                {"filename": "a.rs", "status": "modified", "patch": "@@ -1 +1 @@\n+added"},
                {"filename": "b.png", "status": "added"},
                {"filename": "c.rs", "status": "modified", "patch": "@@ -2 +2 @@\n-old\n+new"}
            ])
            .to_string(),
        );
        let result =
            list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 5})).await;
        assert!(result.is_ok(), "must succeed: {result:?}");
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();

        // a.rs — annotated from position 1
        let patch_a = parsed[0]["patch"].as_str().unwrap();
        assert!(patch_a.starts_with("1 "), "a.rs patch must start at position 1: {patch_a}");

        // b.png — no patch (binary), field absent
        assert!(parsed[1]["patch"].is_null(), "binary file must have no patch");

        // c.rs — annotated independently from position 1 (not continuing from a.rs)
        let patch_c = parsed[2]["patch"].as_str().unwrap();
        assert!(patch_c.starts_with("1 "), "c.rs patch must start at position 1: {patch_c}");
    }

    #[tokio::test]
    async fn list_pr_files_missing_pr_number_returns_err() {
        let ctx = make_ctx();
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r"})).await;
        assert!(result.is_err(), "missing pr_number must return Err: {result:?}");
    }

    #[tokio::test]
    async fn list_pr_files_missing_owner_returns_err() {
        let ctx = make_ctx();
        let result = list_pr_files(&ctx, &json!({"repo": "r", "pr_number": 1})).await;
        assert!(result.is_err(), "missing owner must return Err: {result:?}");
    }

    #[tokio::test]
    async fn list_pr_files_missing_repo_returns_err() {
        let ctx = make_ctx();
        let result = list_pr_files(&ctx, &json!({"owner": "o", "pr_number": 1})).await;
        assert!(result.is_err(), "missing repo must return Err: {result:?}");
    }

    #[tokio::test]
    async fn list_pr_files_http_error_propagates() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_err("connection refused");
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 1})).await;
        assert!(result.is_err(), "HTTP error must propagate: {result:?}");
    }

    #[tokio::test]
    async fn list_pr_files_empty_string_patch_annotated_as_empty() {
        // A file with patch: "" (present but empty) must not crash.
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!([{"filename": "f.rs", "status": "modified", "patch": ""}]).to_string(),
        );
        let result = list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 3})).await;
        assert!(result.is_ok(), "empty patch must succeed: {result:?}");
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed[0]["patch"].as_str().unwrap(), "");
    }

    #[tokio::test]
    async fn post_pr_review_missing_repo_returns_err() {
        let ctx = make_ctx();
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "pr_number": 1, "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_err(), "missing repo must return Err: {result:?}");
        assert!(result.unwrap_err().contains("missing repo"));
    }

    #[tokio::test]
    async fn post_pr_review_missing_pr_number_returns_err() {
        let ctx = make_ctx();
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "body": "x", "event": "COMMENT"}),
        )
        .await;
        assert!(result.is_err(), "missing pr_number must return Err: {result:?}");
        assert!(result.unwrap_err().contains("missing pr_number"));
    }

    #[tokio::test]
    async fn post_pr_review_absent_event_defaults_to_comment() {
        // When `event` is absent the implementation defaults to "COMMENT".
        // The GitHub API accepts this, so the call must succeed.
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"id": 1, "html_url": "https://github.com/o/r/pull/1#pullrequestreview-1"})
                .to_string(),
        );
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "body": "looks good"}),
        )
        .await;
        assert!(result.is_ok(), "absent event must succeed with COMMENT default: {result:?}");
    }

    #[tokio::test]
    async fn post_pr_review_absent_body_defaults_to_empty_string() {
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(
            200,
            json!({"id": 9, "html_url": "https://github.com/o/r/pull/1#pullrequestreview-9"})
                .to_string(),
        );
        let result = post_pr_review(
            &ctx,
            &json!({"owner": "o", "repo": "r", "pr_number": 1, "event": "APPROVE"}),
        )
        .await;
        assert!(result.is_ok(), "absent body must succeed with empty-string default: {result:?}");
    }

    #[tokio::test]
    async fn list_pr_files_non_200_with_valid_array_body_succeeds() {
        // Some GitHub proxies return a 404 status but with a valid JSON array body.
        // The function checks shape (array) not status, so it should succeed and
        // return the (empty) list rather than treating the status as an error.
        let ctx = make_ctx();
        ctx.http_client.enqueue_ok(404, json!([]).to_string());
        let result =
            list_pr_files(&ctx, &json!({"owner": "o", "repo": "r", "pr_number": 99})).await;
        assert!(result.is_ok(), "404 with array body must succeed: {result:?}");
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(parsed.as_array().unwrap().is_empty());
    }

    #[test]
    fn annotate_diff_no_newline_at_eof_marker_is_numbered() {
        // Git emits `\ No newline at end of file` as a real diff line.
        // It must receive a position number like any other line.
        let patch =
            "@@ -1,2 +1,2 @@\n-old\n+new\n\\ No newline at end of file";
        let annotated = annotate_diff(patch);
        let lines: Vec<&str> = annotated.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3], r"4 \ No newline at end of file");
    }

    // ── create_pull_request — existing PR found in pre-check ─────────────────

    /// When the pre-check GET returns a non-empty array, the function must
    /// return early with the existing PR — skipping the POST entirely.
    /// This is the core idempotency path for `create_pull_request`.
    #[tokio::test]
    async fn create_pull_request_precheck_finds_existing_pr_returns_early() {
        let ctx = make_ctx();

        // Pre-check: existing PR found.
        ctx.http_client.enqueue_ok(
            200,
            json!([{
                "number": 55,
                "html_url": "https://github.com/o/r/pull/55"
            }])
            .to_string(),
        );
        // No POST enqueued — the function must return before creating a new PR.

        let result = create_pull_request(
            &ctx,
            &json!({
                "owner": "o", "repo": "r",
                "title": "feat", "head": "feat/branch",
                "_idempotency_key": "pr-dedup-key"
            }),
        )
        .await;

        assert!(result.is_ok(), "must succeed: {result:?}");
        assert!(
            result.unwrap().contains("Pull request #55"),
            "must return the existing PR, not create a new one"
        );
        assert!(
            ctx.http_client.is_empty(),
            "no POST must be made when an existing PR is found"
        );
    }

    // ── request_reviewers — GET connection error ──────────────────────────────

    /// When the pre-check GET fails with a network error, `request_reviewers`
    /// must degrade gracefully and request all reviewers (line: `Err(_) => reviewers.clone()`).
    #[tokio::test]
    async fn request_reviewers_get_connection_error_falls_back_to_all_reviewers() {
        let ctx = make_ctx();

        // GET fails — connection refused.
        ctx.http_client.enqueue_err("connection refused");

        // POST with all reviewers must follow.
        ctx.http_client
            .enqueue_ok(201, json!({"number": 12}).to_string());

        let result = request_reviewers(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 12,
                "reviewers": ["alice", "bob"],
                "_idempotency_key": "get-error-key"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "must succeed after GET connection error: {result:?}"
        );
        assert!(result.unwrap().contains("Reviewers requested on PR #12"));
        assert!(
            ctx.http_client.is_empty(),
            "both GET (failed) and POST must have been consumed"
        );
    }

    // ── post_pr_comment — idempotency key, partial page, no marker → POST ────

    /// When an idempotency key is present and the first (and only) page returns
    /// fewer than 100 items without the marker, the dedup loop must break early
    /// and proceed with the POST — the normal recovery scenario.
    #[tokio::test]
    async fn post_pr_comment_idempotency_key_partial_page_no_marker_posts() {
        let ctx = make_ctx();

        // One page with 3 comments — none containing the marker.
        ctx.http_client.enqueue_ok(
            200,
            json!([
                {"id": 1, "body": "First comment", "html_url": "https://github.com/o/r/issues/1#issuecomment-1"},
                {"id": 2, "body": "Second comment", "html_url": "https://github.com/o/r/issues/1#issuecomment-2"},
                {"id": 3, "body": "Third comment", "html_url": "https://github.com/o/r/issues/1#issuecomment-3"}
            ])
            .to_string(),
        );

        // POST — the actual comment creation.
        ctx.http_client.enqueue_ok(
            201,
            json!({"id": 99, "html_url": "https://github.com/o/r/issues/1#issuecomment-99"})
                .to_string(),
        );

        let result = post_pr_comment(
            &ctx,
            &json!({
                "owner": "o", "repo": "r", "pr_number": 1,
                "body": "New review comment",
                "_idempotency_key": "partial-page-key"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "must succeed after partial-page dedup scan: {result:?}"
        );
        assert!(
            result.unwrap().contains("Comment posted"),
            "must return a 'Comment posted' message"
        );
        assert!(
            ctx.http_client.is_empty(),
            "both GET and POST must have been consumed"
        );
    }

    // ── get_file_contents — missing path field ────────────────────────────────

    #[tokio::test]
    async fn get_file_contents_missing_path_returns_err() {
        let ctx = make_ctx();
        let result = get_file_contents(
            &ctx,
            &json!({"owner": "o", "repo": "r"}), // "path" intentionally absent
        )
        .await;
        assert!(result.is_err(), "missing path must return Err: {result:?}");
        assert!(
            result.unwrap_err().contains("missing path"),
            "error must name the missing field"
        );
    }
}
