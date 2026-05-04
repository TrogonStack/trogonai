use serde::{Deserialize, Serialize};
use trogon_actor::{ActorContext, EntityActor};

// ── HttpClient trait ──────────────────────────────────────────────────────────

/// Abstraction over HTTP so unit tests can inject a mock without real networking.
///
/// Production code uses `reqwest::Client`; tests use [`MockHttpClient`].
pub trait HttpClient: Clone + Send + Sync + 'static {
    /// GET request. Returns `(status_code, body_text)` on any HTTP response;
    /// errors only on network failure.
    fn get(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> impl std::future::Future<Output = Result<(u16, String), String>> + Send;

    /// POST request with a JSON body. Returns `(status_code, body_text)`.
    fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<(u16, String), String>> + Send;
}

impl HttpClient for reqwest::Client {
    async fn get(&self, url: &str, headers: Vec<(String, String)>) -> Result<(u16, String), String> {
        let mut req = self.get(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        Ok((status, body))
    }

    async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Result<(u16, String), String> {
        let mut req = self.post(url).json(&body);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}

// ── State ──────────────────────────────────────────────────────────────────────

/// Persisted state for a single pull request, accumulating across all events.
///
/// The runtime loads this before every event and saves it after — giving the
/// actor continuous memory of everything that happened since the PR opened.
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct PrState {
    /// Total number of events processed for this PR.
    pub events_processed: u32,
    /// SHA of the most recently reviewed head commit, if any.
    pub last_reviewed_sha: Option<String>,
    /// Issues identified and tracked across all review sessions.
    pub issues_found: Vec<String>,
    /// Number of review comments posted to GitHub for this PR.
    pub comments_posted: u32,
}

// ── Actor ─────────────────────────────────────────────────────────────────────

/// Entity Actor for the full lifecycle of a GitHub pull request.
///
/// Subscribes to `actors.pr.>` via [`trogon_actor::host::ActorHost`]. Entity
/// keys follow the pattern `{owner}.{repo}.{number}`, e.g.
/// `acme.myrepo.42` (NATS-safe dots instead of slashes).
///
/// Each event — PR opened, commit pushed, CI result, review requested — is
/// dispatched here by the router. The actor:
/// 1. Fetches the current PR diff from the GitHub API.
/// 2. Calls the LLM to produce a structured code review.
/// 3. Posts a review comment to the PR.
/// 4. Accumulates state so later events have full context from earlier ones.
///
/// If the LLM API key or GitHub token is not configured the actor degrades
/// gracefully: it still increments the event counter and writes transcript
/// entries, but skips the LLM and GitHub calls.
///
/// Generic over `H: HttpClient` so unit tests can inject a [`MockHttpClient`]
/// without real networking. Production uses `reqwest::Client`.
#[derive(Clone)]
pub struct PrActor<H: HttpClient = reqwest::Client> {
    http: H,
    llm_api_url: String,
    llm_api_key: String,
    llm_model: String,
    github_token: String,
    github_api_url: String,
}

impl Default for PrActor<reqwest::Client> {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
            llm_api_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            github_token: String::new(),
            github_api_url: "https://api.github.com".to_string(),
        }
    }
}

impl<H: HttpClient> PrActor<H> {
    /// Construct an actor with explicit credentials.
    pub fn new(
        http: H,
        llm_api_url: impl Into<String>,
        llm_api_key: impl Into<String>,
        llm_model: impl Into<String>,
        github_token: impl Into<String>,
    ) -> Self {
        Self {
            http,
            llm_api_url: llm_api_url.into(),
            llm_api_key: llm_api_key.into(),
            llm_model: llm_model.into(),
            github_token: github_token.into(),
            github_api_url: "https://api.github.com".to_string(),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Parse `"owner.repo.42"` into `("owner", "repo", 42)`.
    ///
    /// Assumes the last dot-segment is the PR number and everything before the
    /// first dot is the owner. For multi-segment repo names (e.g.
    /// `"org.sub.repo.42"`) the middle segments are joined back with `.` for
    /// the repo name portion.
    fn parse_entity_key(entity_key: &str) -> Option<(String, String, u64)> {
        let parts: Vec<&str> = entity_key.splitn(3, '.').collect();
        if parts.len() != 3 {
            return None;
        }
        let owner = parts[0].to_string();
        let repo_and_number = parts[2];

        // Two cases:
        //   "owner.repo.42"         → parts[2]="42"         (no dot → simple case)
        //   "owner.sub.repo.42"     → parts[2]="repo.42"    (dot present)
        if let Some((repo_part, number_str)) = repo_and_number.rsplit_once('.') {
            let number: u64 = number_str.parse().ok()?;
            let repo = if repo_part.is_empty() {
                parts[1].to_string()
            } else {
                format!("{}.{}", parts[1], repo_part)
            };
            Some((owner, repo, number))
        } else {
            // Simple "owner.repo.number" — parts[2] is purely the PR number.
            let number: u64 = repo_and_number.parse().ok()?;
            Some((owner, parts[1].to_string(), number))
        }
    }

    /// Fetch the HEAD commit SHA and unified diff for a GitHub pull request.
    ///
    /// Returns `(head_sha, diff_text)`. The diff uses
    /// `Accept: application/vnd.github.diff`.
    async fn fetch_pr_diff_and_sha(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<(String, String), String> {
        let url = format!("{}/repos/{owner}/{repo}/pulls/{number}", self.github_api_url);
        let auth = format!("Bearer {}", self.github_token);

        // First call: JSON to get HEAD SHA.
        let (_, info_text) = self
            .http
            .get(
                &url,
                vec![
                    ("Accept".to_string(), "application/vnd.github+json".to_string()),
                    ("Authorization".to_string(), auth.clone()),
                    ("User-Agent".to_string(), "trogon-pr-actor/0.1".to_string()),
                ],
            )
            .await
            .map_err(|e| format!("GitHub API request failed: {e}"))?;

        let info: serde_json::Value = serde_json::from_str(&info_text)
            .map_err(|e| format!("parsing PR info JSON failed: {e}"))?;

        let head_sha = info
            .pointer("/head/sha")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Second call: raw diff.
        let (status, diff_body) = self
            .http
            .get(
                &url,
                vec![
                    ("Accept".to_string(), "application/vnd.github.diff".to_string()),
                    ("Authorization".to_string(), auth),
                    ("User-Agent".to_string(), "trogon-pr-actor/0.1".to_string()),
                ],
            )
            .await
            .map_err(|e| format!("GitHub API request failed: {e}"))?;

        if !(200u16..300).contains(&status) {
            return Err(format!("GitHub API returned {status}: {diff_body}"));
        }

        Ok((head_sha, diff_body))
    }

    /// Ask the LLM to review the diff and return a list of issues found.
    async fn review_with_llm(
        &self,
        entity_key: &str,
        events_processed: u32,
        diff: &str,
    ) -> Result<Vec<String>, String> {
        let url = format!("{}/chat/completions", self.llm_api_url);

        // Truncate very large diffs to avoid context overflow.
        let diff_preview = if diff.len() > 20_000 {
            &diff[..20_000]
        } else {
            diff
        };

        let prompt = format!(
            "You are a senior engineer performing a code review on pull request `{entity_key}` \
             (event #{events_processed}).\n\n\
             Respond with a JSON object: \
             {{\"issues\": [\"issue 1\", \"issue 2\", ...], \"summary\": \"one-line summary\"}}.\n\
             Return an empty array if the diff looks clean.\n\n\
             --- DIFF ---\n{diff_preview}\n--- END DIFF ---"
        );

        let body = serde_json::json!({
            "model": self.llm_model,
            "messages": [{"role": "system", "content": prompt}],
            "response_format": {"type": "json_object"},
        });

        let (status, text) = self
            .http
            .post_json(
                &url,
                vec![("Authorization".to_string(), format!("Bearer {}", self.llm_api_key))],
                body,
            )
            .await
            .map_err(|e| format!("LLM request failed: {e}"))?;

        if !(200u16..300).contains(&status) {
            return Err(format!("LLM returned {status}: {text}"));
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("parsing LLM response: {e}"))?;

        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("unexpected LLM response shape: {json}"))?;

        let review: serde_json::Value =
            serde_json::from_str(content).map_err(|e| format!("parsing review JSON: {e}"))?;

        let issues = review["issues"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        Ok(issues)
    }

    /// Post a review comment to a GitHub pull request.
    async fn post_review_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), String> {
        let url = format!("{}/repos/{owner}/{repo}/pulls/{number}/reviews", self.github_api_url);

        let payload = serde_json::json!({
            "body": body,
            "event": "COMMENT",
        });

        let (status, resp_body) = self
            .http
            .post_json(
                &url,
                vec![
                    ("Authorization".to_string(), format!("Bearer {}", self.github_token)),
                    ("Accept".to_string(), "application/vnd.github+json".to_string()),
                    ("User-Agent".to_string(), "trogon-pr-actor/0.1".to_string()),
                ],
                payload,
            )
            .await
            .map_err(|e| format!("GitHub post review failed: {e}"))?;

        if !(200u16..300).contains(&status) {
            return Err(format!("GitHub post review returned {status}: {resp_body}"));
        }

        Ok(())
    }
}

// ── EntityActor impl ──────────────────────────────────────────────────────────

impl<H: HttpClient> EntityActor for PrActor<H> {
    type State = PrState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "pr"
    }

    async fn handle(&mut self, state: &mut PrState, ctx: &ActorContext) -> Result<(), Self::Error> {
        state.events_processed += 1;

        tracing::info!(
            entity_key = %ctx.entity_key,
            events = state.events_processed,
            issues = state.issues_found.len(),
            "PR event received"
        );

        // Record the incoming event in the transcript.
        let user_msg = format!(
            "PR event #{n} for `{key}`.",
            n = state.events_processed,
            key = ctx.entity_key,
        );
        ctx.append_user_message(&user_msg, None).await.ok();

        // ── GitHub + LLM integration ──────────────────────────────────────────
        //
        // Only runs when both credentials are configured. Degrades gracefully
        // when running without credentials (e.g. in tests or during bootstrap).

        let mut review_summary = format!("Acknowledged event #{n}.", n = state.events_processed);

        if !self.github_token.is_empty() && !self.llm_api_key.is_empty() {
            if let Some((owner, repo, number)) = Self::parse_entity_key(&ctx.entity_key) {
                match self.fetch_pr_diff_and_sha(&owner, &repo, number).await {
                    Ok((head_sha, diff)) => {
                        state.last_reviewed_sha = Some(head_sha);
                        match self
                            .review_with_llm(&ctx.entity_key, state.events_processed, &diff)
                            .await
                        {
                            Ok(issues) => {
                                // Update state with newly discovered issues.
                                for issue in &issues {
                                    if !state.issues_found.contains(issue) {
                                        state.issues_found.push(issue.clone());
                                    }
                                }

                                // Build and post the review comment.
                                if !issues.is_empty() {
                                    let comment = format!(
                                        "## Automated Code Review (event #{n})\n\n{items}",
                                        n = state.events_processed,
                                        items = issues
                                            .iter()
                                            .map(|i| format!("- {i}"))
                                            .collect::<Vec<_>>()
                                            .join("\n"),
                                    );
                                    match self
                                        .post_review_comment(&owner, &repo, number, &comment)
                                        .await
                                    {
                                        Ok(()) => {
                                            state.comments_posted += 1;
                                            review_summary = format!(
                                                "Posted review for event #{n}: {count} issue(s) found.",
                                                n = state.events_processed,
                                                count = issues.len(),
                                            );
                                            tracing::info!(
                                                entity_key = %ctx.entity_key,
                                                issues = issues.len(),
                                                "review comment posted"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                entity_key = %ctx.entity_key,
                                                error = %e,
                                                "failed to post review comment"
                                            );
                                            review_summary = format!(
                                                "LLM review done but comment post failed: {e}"
                                            );
                                        }
                                    }
                                } else {
                                    review_summary = format!(
                                        "Event #{n}: no issues found.",
                                        n = state.events_processed
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    entity_key = %ctx.entity_key,
                                    error = %e,
                                    "LLM review failed"
                                );
                                review_summary = format!("LLM review failed: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            entity_key = %ctx.entity_key,
                            error = %e,
                            "failed to fetch PR diff"
                        );
                        review_summary = format!("Diff fetch failed: {e}");
                    }
                }
            } else {
                tracing::warn!(
                    entity_key = %ctx.entity_key,
                    "entity key does not match owner.repo.number pattern — skipping review"
                );
            }
        }

        ctx.append_assistant_message(&review_summary, None)
            .await
            .ok();

        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use trogon_actor::ContextBuilder;

    // ── MockHttpClient ────────────────────────────────────────────────────────

    /// In-memory mock for [`HttpClient`]. Returns pre-programmed
    /// `(status, body)` responses in FIFO order. Panics if the queue is empty.
    #[derive(Clone)]
    struct MockHttpClient {
        responses: Arc<Mutex<VecDeque<(u16, String)>>>,
    }

    impl MockHttpClient {
        fn new() -> Self {
            Self {
                responses: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn push(&self, status: u16, body: impl Into<String>) {
            self.responses.lock().unwrap().push_back((status, body.into()));
        }
    }

    impl HttpClient for MockHttpClient {
        async fn get(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
        ) -> Result<(u16, String), String> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "MockHttpClient: no response queued for get".to_string())
        }

        async fn post_json(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: serde_json::Value,
        ) -> Result<(u16, String), String> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "MockHttpClient: no response queued for post_json".to_string())
        }
    }

    fn actor() -> PrActor<reqwest::Client> {
        PrActor::default() // no credentials — skips GitHub/LLM calls
    }

    fn mock_actor(http: MockHttpClient) -> PrActor<MockHttpClient> {
        PrActor::new(http, "http://llm", "llm-key", "test-model", "gh-token")
    }

    #[tokio::test]
    async fn actor_type_is_pr() {
        assert_eq!(PrActor::<reqwest::Client>::actor_type(), "pr");
    }

    #[tokio::test]
    async fn handle_increments_events_processed() {
        let (ctx, _entries) = ContextBuilder::new("pr", "acme.repo.1").build();
        let mut state = PrState::default();
        let mut actor = actor();

        actor.handle(&mut state, &ctx).await.unwrap();
        assert_eq!(state.events_processed, 1);

        actor.handle(&mut state, &ctx).await.unwrap();
        assert_eq!(state.events_processed, 2);
    }

    #[tokio::test]
    async fn handle_appends_two_transcript_entries() {
        let (ctx, entries) = ContextBuilder::new("pr", "acme.repo.42").build();
        let mut state = PrState::default();
        actor().handle(&mut state, &ctx).await.unwrap();

        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 2, "expected user + assistant entries");
    }

    #[tokio::test]
    async fn on_create_default_is_noop() {
        let mut state = PrState::default();
        PrActor::<reqwest::Client>::on_create(&mut state).await.unwrap();
        assert_eq!(state.events_processed, 0);
    }

    #[tokio::test]
    async fn state_default_is_zeroed() {
        let s = PrState::default();
        assert_eq!(s.events_processed, 0);
        assert!(s.last_reviewed_sha.is_none());
        assert!(s.issues_found.is_empty());
        assert_eq!(s.comments_posted, 0);
    }

    #[test]
    fn parse_entity_key_standard_format() {
        let result = PrActor::<reqwest::Client>::parse_entity_key("anthropics.my-repo.42");
        assert_eq!(
            result,
            Some(("anthropics".to_string(), "my-repo".to_string(), 42))
        );
    }

    #[test]
    fn parse_entity_key_invalid_returns_none() {
        assert!(PrActor::<reqwest::Client>::parse_entity_key("missing-number").is_none());
        assert!(PrActor::<reqwest::Client>::parse_entity_key("only.two").is_none());
    }

    #[test]
    fn parse_entity_key_non_numeric_pr_returns_none() {
        assert!(PrActor::<reqwest::Client>::parse_entity_key("org.repo.notanumber").is_none());
    }

    #[test]
    fn parse_entity_key_multi_segment_repo() {
        let result = PrActor::<reqwest::Client>::parse_entity_key("acme.infra.my-repo.7");
        assert_eq!(
            result,
            Some(("acme".to_string(), "infra.my-repo".to_string(), 7))
        );
    }

    // ── HTTP error paths ──────────────────────────────────────────────────────

    mod http_errors {
        use super::*;

        #[tokio::test]
        async fn fetch_pr_diff_returns_err_on_404() {
            let http = MockHttpClient::new();
            // First GET (JSON info): 404 with valid JSON body — .json() succeeds, sha = ""
            http.push(404, r#"{"message":"Not Found"}"#);
            // Second GET (diff): 404 — triggers the !is_success() error path
            http.push(404, "Not Found");

            let actor = mock_actor(http);
            let result = actor.fetch_pr_diff_and_sha("owner", "repo", 1).await;
            assert!(result.is_err(), "expected Err on 404, got Ok");
            let msg = result.unwrap_err();
            assert!(msg.contains("404"), "error should mention status 404, got: {msg}");
        }

        #[tokio::test]
        async fn fetch_pr_diff_returns_err_on_500() {
            let http = MockHttpClient::new();
            // First GET returns non-JSON body → JSON parse fails → Err
            http.push(500, "internal server error");

            let actor = mock_actor(http);
            let result = actor.fetch_pr_diff_and_sha("owner", "repo", 2).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn review_with_llm_returns_err_on_500() {
            let http = MockHttpClient::new();
            http.push(500, "overloaded");

            let actor = mock_actor(http);
            let result = actor.review_with_llm("owner.repo.1", 1, "diff text").await;
            assert!(result.is_err());
            let msg = result.unwrap_err();
            assert!(msg.contains("500"), "error should mention status 500, got: {msg}");
        }

        #[tokio::test]
        async fn review_with_llm_returns_err_on_missing_choices_field() {
            let http = MockHttpClient::new();
            http.push(200, r#"{"result":"ok"}"#);

            let actor = mock_actor(http);
            let result = actor.review_with_llm("owner.repo.1", 1, "diff").await;
            assert!(result.is_err(), "expected Err for missing choices path");
        }

        #[tokio::test]
        async fn review_with_llm_returns_err_when_content_is_not_valid_json() {
            let http = MockHttpClient::new();
            http.push(
                200,
                r#"{"choices":[{"message":{"content":"not json {"}}]}"#,
            );

            let actor = mock_actor(http);
            let result = actor.review_with_llm("owner.repo.1", 1, "diff").await;
            assert!(result.is_err(), "expected Err for invalid content JSON");
        }

        #[tokio::test]
        async fn post_review_comment_returns_err_on_422() {
            let http = MockHttpClient::new();
            http.push(422, "validation failed");

            let actor = mock_actor(http);
            let result = actor.post_review_comment("owner", "repo", 3, "body text").await;
            assert!(result.is_err());
            let msg = result.unwrap_err();
            assert!(msg.contains("422"), "error should mention 422, got: {msg}");
        }

        #[tokio::test]
        async fn post_review_comment_ok_on_200() {
            let http = MockHttpClient::new();
            http.push(200, "{}");

            let actor = mock_actor(http);
            let result = actor.post_review_comment("owner", "repo", 4, "LGTM").await;
            assert!(result.is_ok());
        }
    }
}
