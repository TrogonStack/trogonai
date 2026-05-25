use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt as _;

const TIMEOUT: Duration = Duration::from_secs(120);

#[async_trait]
pub trait SpawnHttpClient: Send + Sync {
    async fn complete(&self, api_key: &str, model: &str, prompt: &str, url: &str) -> String;
}

#[derive(Clone)]
pub struct ReqwestSpawnClient;

#[async_trait]
impl SpawnHttpClient for ReqwestSpawnClient {
    async fn complete(&self, api_key: &str, model: &str, prompt: &str, url: &str) -> String {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let result = tokio::time::timeout(TIMEOUT, async {
            client
                .post(url)
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
}

/// Subscribes to `{prefix}.agent.spawn` on the NATS queue group `"spawn-handlers"`
/// and handles each request by calling the injected HTTP client.
///
/// Runs until the NATS connection drops or the subscriber is closed. Spawn this
/// in a `tokio::spawn` from `main`.
pub async fn run_spawn_subscriber<C: SpawnHttpClient + Send + Sync + 'static>(
    nats: async_nats::Client,
    prefix: String,
    api_key: String,
    model: String,
    base_url: String,
    client: Arc<C>,
) {
    let mut sub = nats
        .queue_subscribe(format!("{prefix}.agent.spawn"), "spawn-handlers".to_string())
        .await
        .expect("failed to subscribe to agent.spawn");
    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else { continue };
        // MED-33: a malformed payload must still send a reply, otherwise the
        // requester blocks on its NATS request-reply until the spawn timeout.
        let req = match serde_json::from_slice::<serde_json::Value>(&msg.payload) {
            Ok(req) => req,
            Err(e) => {
                nats.publish(reply, format!("error: invalid spawn request: {e}").into())
                    .await
                    .ok();
                continue;
            }
        };
        let prompt = req["prompt"].as_str().unwrap_or("").to_string();
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let nats2 = nats.clone();
        let key2 = api_key.clone();
        let model2 = model.clone();
        let client2 = Arc::clone(&client);
        tokio::spawn(async move {
            let result = oneshot_call_with_client(client2.as_ref(), &key2, &model2, &prompt, &url).await;
            nats2.publish(reply, result.into()).await.ok();
        });
    }
}

/// One-shot xAI call: no session, no history, no tools.
/// Returns the assistant text or an error string.
pub async fn oneshot_call(api_key: &str, model: &str, prompt: &str) -> String {
    let base_url = std::env::var("XAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    oneshot_call_with_client(&ReqwestSpawnClient, api_key, model, prompt, &url).await
}

async fn oneshot_call_with_client<C: SpawnHttpClient>(
    client: &C,
    api_key: &str,
    model: &str,
    prompt: &str,
    url: &str,
) -> String {
    client.complete(api_key, model, prompt, url).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockSpawnClient {
        response: String,
        captured: Arc<Mutex<Option<(String, String, String, String)>>>,
    }

    impl MockSpawnClient {
        fn responding(
            response: impl Into<String>,
        ) -> (Self, Arc<Mutex<Option<(String, String, String, String)>>>) {
            let captured = Arc::new(Mutex::new(None));
            (
                Self { response: response.into(), captured: Arc::clone(&captured) },
                captured,
            )
        }
    }

    #[async_trait]
    impl SpawnHttpClient for MockSpawnClient {
        async fn complete(&self, api_key: &str, model: &str, prompt: &str, url: &str) -> String {
            *self.captured.lock().unwrap() =
                Some((api_key.to_string(), model.to_string(), prompt.to_string(), url.to_string()));
            self.response.clone()
        }
    }

    // ── response-side ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn returns_assistant_content_on_success() {
        let (mock, _) = MockSpawnClient::responding("hello from xai");
        let result = oneshot_call_with_client(&mock, "key", "grok-4", "hi", "http://x/chat/completions").await;
        assert_eq!(result, "hello from xai");
    }

    #[tokio::test]
    async fn returns_error_on_non_200() {
        let (mock, _) = MockSpawnClient::responding("error: API returned 401");
        let result = oneshot_call_with_client(&mock, "key", "grok-4", "hi", "http://x/chat/completions").await;
        assert!(result.starts_with("error: API returned 401"), "got: {result}");
    }

    #[tokio::test]
    async fn returns_error_on_invalid_json() {
        let (mock, _) = MockSpawnClient::responding("error parsing response: invalid");
        let result = oneshot_call_with_client(&mock, "key", "grok-4", "hi", "http://x/chat/completions").await;
        assert!(result.starts_with("error parsing response:"), "got: {result}");
    }

    #[tokio::test]
    async fn returns_empty_string_when_content_missing() {
        let (mock, _) = MockSpawnClient::responding("");
        let result = oneshot_call_with_client(&mock, "key", "grok-4", "hi", "http://x/chat/completions").await;
        assert_eq!(result, "");
    }

    // ── request payload ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn sends_correct_model_and_prompt() {
        let (mock, captured) = MockSpawnClient::responding("ok");
        oneshot_call_with_client(&mock, "key", "grok-4", "explain recursion", "http://x/chat/completions").await;
        let (_, model, prompt, _) = captured.lock().unwrap().take().unwrap();
        assert_eq!(model, "grok-4");
        assert_eq!(prompt, "explain recursion");
    }

    #[tokio::test]
    async fn sends_bearer_auth_header() {
        let (mock, captured) = MockSpawnClient::responding("ok");
        oneshot_call_with_client(&mock, "my-secret-key", "grok-4", "hi", "http://x/chat/completions").await;
        let (api_key, _, _, _) = captured.lock().unwrap().take().unwrap();
        assert_eq!(api_key, "my-secret-key");
    }
}
