use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(120);

/// One-shot xAI call: no session, no history, no tools.
/// Returns the assistant text or an error string.
pub async fn oneshot_call(api_key: &str, model: &str, prompt: &str) -> String {
    let base_url = std::env::var("XAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());

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
