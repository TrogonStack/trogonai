use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(120);

/// One-shot xAI call: no session, no history, no tools.
/// Returns the assistant text or an error string.
pub async fn oneshot_call(api_key: &str, model: &str, prompt: &str) -> String {
    let base_url = std::env::var("XAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
    oneshot_call_with_url(api_key, model, prompt, &base_url).await
}

async fn oneshot_call_with_url(api_key: &str, model: &str, prompt: &str, base_url: &str) -> String {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let result = tokio::time::timeout(TIMEOUT, async {
        client
            .post(&url)
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}]
            }))
            .send()
            .await
    })
    .await;

    match result {
        Ok(Ok(resp)) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => json["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                Err(e) => format!("error parsing response: {e}"),
            }
        }
        Ok(Ok(resp)) => format!("error: API returned {}", resp.status()),
        Ok(Err(e)) => format!("error: {e}"),
        Err(_) => format!("error: oneshot_call timed out after {}s", TIMEOUT.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use axum::Router;
    use axum::routing::post;
    use axum::response::IntoResponse;
    use axum::extract::{Json, State};
    use axum::http::HeaderMap;

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
        format!("http://{addr}")
    }

    fn ok_response() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        }))
    }

    // ── response-side ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn returns_assistant_content_on_success() {
        let app = Router::new().route("/chat/completions", post(|| async {
            axum::Json(serde_json::json!({
                "choices": [{"message": {"content": "hello from xai"}}]
            }))
            .into_response()
        }));
        let base = serve(app).await;
        let result = oneshot_call_with_url("key", "grok-4", "hi", &base).await;
        assert_eq!(result, "hello from xai");
    }

    #[tokio::test]
    async fn returns_error_on_non_200() {
        let app = Router::new().route("/chat/completions", post(|| async {
            (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
        }));
        let base = serve(app).await;
        let result = oneshot_call_with_url("bad-key", "grok-4", "hi", &base).await;
        assert!(result.starts_with("error: API returned 401"), "got: {result}");
    }

    #[tokio::test]
    async fn returns_error_on_invalid_json() {
        let app = Router::new().route("/chat/completions", post(|| async {
            (axum::http::StatusCode::OK, "not json at all").into_response()
        }));
        let base = serve(app).await;
        let result = oneshot_call_with_url("key", "grok-4", "hi", &base).await;
        assert!(result.starts_with("error parsing response:"), "got: {result}");
    }

    #[tokio::test]
    async fn returns_empty_string_when_content_missing() {
        let app = Router::new().route("/chat/completions", post(|| async {
            axum::Json(serde_json::json!({"choices": []})).into_response()
        }));
        let base = serve(app).await;
        let result = oneshot_call_with_url("key", "grok-4", "hi", &base).await;
        assert_eq!(result, "");
    }

    // ── request payload ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn sends_correct_model_and_prompt() {
        let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/chat/completions", post(
                |State(cap): State<Arc<Mutex<Option<serde_json::Value>>>>,
                 Json(body): Json<serde_json::Value>| async move {
                    *cap.lock().unwrap() = Some(body);
                    ok_response().into_response()
                }
            ))
            .with_state(captured.clone());
        let base = serve(app).await;

        oneshot_call_with_url("key", "grok-4", "explain recursion", &base).await;

        let body = captured.lock().unwrap().take().unwrap();
        assert_eq!(body["model"].as_str(), Some("grok-4"));
        assert_eq!(body["messages"][0]["role"].as_str(), Some("user"));
        assert_eq!(body["messages"][0]["content"].as_str(), Some("explain recursion"));
    }

    #[tokio::test]
    async fn sends_bearer_auth_header() {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/chat/completions", post(
                |State(cap): State<Arc<Mutex<Option<String>>>>,
                 headers: HeaderMap| async move {
                    let auth = headers.get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    *cap.lock().unwrap() = auth;
                    ok_response().into_response()
                }
            ))
            .with_state(captured.clone());
        let base = serve(app).await;

        oneshot_call_with_url("my-secret-key", "grok-4", "hi", &base).await;

        let auth = captured.lock().unwrap().take().unwrap();
        assert_eq!(auth, "Bearer my-secret-key");
    }
}
