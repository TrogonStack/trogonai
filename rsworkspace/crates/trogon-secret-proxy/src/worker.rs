//! JetStream pull-consumer worker: detokenizes and forwards HTTP requests.
//!
//! Each running instance:
//! 1. Pulls [`OutboundHttpRequest`] messages from the `PROXY_REQUESTS` stream.
//! 2. Extracts the `tok_...` token from the `Authorization` header.
//! 3. Resolves the real API key via the [`VaultStore`].
//! 4. Forwards the HTTP request to the AI provider using [`HttpClient`].
//! 5. Publishes an [`OutboundHttpResponse`] to the Core NATS reply subject.
//! 6. Acknowledges the JetStream message.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use trogon_vault::VaultStore;

use crate::messages::{OutboundHttpRequest, OutboundHttpResponse};
use crate::traits::{HttpClient, HttpResponse, JetStreamConsumerClient, NatsClient};

const BEARER_PREFIX: &str = "Bearer ";

/// Maximum number of times to retry a transient upstream failure.
const HTTP_MAX_RETRIES: u32 = 3;
/// Initial backoff delay; doubles on each subsequent attempt.
const HTTP_INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Run the detokenization worker loop.
///
/// Pull messages from JetStream, resolve tokens, forward to AI providers.
/// Runs until the process exits or NATS disconnects.
pub async fn run<V, J, N, H>(
    jetstream: J,
    nats: N,
    vault: Arc<V>,
    http_client: H,
    consumer_name: &str,
    stream_name: &str,
) -> Result<(), WorkerError>
where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
    J: JetStreamConsumerClient,
    N: NatsClient,
    H: HttpClient,
{
    let mut messages = jetstream
        .get_messages(stream_name, consumer_name)
        .await
        .map_err(WorkerError::JetStream)?;

    tracing::info!(consumer = %consumer_name, "Worker started, pulling messages");

    while let Some(msg_result) = messages.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "Error receiving JetStream message");
                continue;
            }
        };

        let subject = msg.subject.clone();
        tracing::debug!(subject = %subject, "Received JetStream message");

        let request: OutboundHttpRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize OutboundHttpRequest; nacking");
                msg.nack().await;
                continue;
            }
        };

        let reply_to = request.reply_to.clone();

        let response = process_request(&request, &*vault, &http_client).await;

        let payload = match serde_json::to_vec(&response) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize OutboundHttpResponse");
                msg.nack().await;
                continue;
            }
        };

        if let Err(e) = nats.publish(reply_to.clone(), payload.into()).await {
            tracing::error!(
                reply_to = %reply_to,
                error = %e,
                "Failed to publish reply to Core NATS"
            );
            // Publish a compact error response so the proxy returns a
            // descriptive error to the caller instead of timing out with 504.
            let error_response = OutboundHttpResponse {
                status: 502,
                headers: vec![],
                body: vec![],
                error: Some(format!("Worker failed to publish reply: {}", e)),
            };
            if let Ok(error_payload) = serde_json::to_vec(&error_response) {
                let _ = nats.publish(reply_to.clone(), error_payload.into()).await;
            }
        }

        msg.ack().await;
    }

    Ok(())
}

/// Process a single outbound request: detokenize and call the AI provider.
async fn process_request<V, H>(
    request: &OutboundHttpRequest,
    vault: &V,
    http_client: &H,
) -> OutboundHttpResponse
where
    V: VaultStore,
    V::Error: std::fmt::Display,
    H: HttpClient,
{
    let real_key = match resolve_token(vault, &request.headers).await {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(error = %e, "Token resolution failed");
            return OutboundHttpResponse {
                status: 401,
                headers: vec![],
                body: vec![],
                error: Some(e),
            };
        }
    };

    // Build the forwarded header list:
    // - strip any existing Authorization and X-Request-Id from the client
    // - add the resolved real key and the idempotency key
    let mut forwarded_headers: Vec<(String, String)> = request
        .headers
        .iter()
        .filter(|(k, _)| {
            !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("x-request-id")
        })
        .cloned()
        .collect();
    forwarded_headers.push(("Authorization".to_string(), format!("Bearer {}", real_key)));
    forwarded_headers.push(("X-Request-Id".to_string(), request.idempotency_key.clone()));

    match forward_request_with_retry(http_client, request, &forwarded_headers).await {
        Ok(mut resp) => {
            // Strip any response header whose value contains the real API key.
            // A misbehaving provider might echo it back; the caller must never
            // see it regardless.
            resp.headers.retain(|(_, v)| !v.contains(real_key.as_str()));
            resp
        }
        Err(e) => {
            tracing::error!(error = %e, url = %request.url, "Upstream HTTP call failed after retries");
            OutboundHttpResponse {
                status: 502,
                headers: vec![],
                body: vec![],
                error: Some(e),
            }
        }
    }
}

/// Extract the `tok_...` token from the Authorization header and resolve it.
async fn resolve_token<V>(vault: &V, headers: &[(String, String)]) -> Result<String, String>
where
    V: VaultStore,
    V::Error: std::fmt::Display,
{
    let auth_value = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| "Missing Authorization header".to_string())?;

    if !auth_value.starts_with(BEARER_PREFIX) {
        return Err("Authorization header is not a Bearer token".to_string());
    }

    let raw_token = &auth_value[BEARER_PREFIX.len()..];

    if !raw_token.starts_with("tok_") {
        return Err(format!(
            "Authorization token does not look like a proxy token (expected tok_..., got {})",
            &raw_token[..raw_token.len().min(16)]
        ));
    }

    let token = trogon_vault::ApiKeyToken::new(raw_token)
        .map_err(|e| format!("Invalid proxy token: {}", e))?;

    let real_key = vault
        .resolve(&token)
        .await
        .map_err(|e| format!("Vault error: {}", e))?
        .ok_or_else(|| format!("Token not found in vault: {}", token))?;

    if real_key.is_empty() {
        return Err(format!("Vault returned an empty key for token: {}", token));
    }

    Ok(real_key)
}

/// Retry wrapper around [`forward_request`] using exponential backoff.
///
/// Retries on:
/// - Network/transport errors (connection refused, timeout, etc.)
/// - 5xx responses from the upstream provider (transient server errors)
///
/// Does **not** retry on 4xx responses (client errors — retrying won't help).
async fn forward_request_with_retry<H>(
    http_client: &H,
    request: &OutboundHttpRequest,
    headers: &[(String, String)],
) -> Result<OutboundHttpResponse, String>
where
    H: HttpClient,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match forward_request(http_client, request, headers).await {
            // Success or non-retryable client error — return immediately.
            Ok(resp) if resp.status < 500 => return Ok(resp),
            // 5xx but retries exhausted.
            Ok(resp) if attempts > HTTP_MAX_RETRIES => {
                tracing::warn!(
                    url = %request.url,
                    status = resp.status,
                    attempts,
                    "Upstream returned 5xx after all retries"
                );
                return Ok(resp);
            }
            // 5xx — backoff and retry.
            Ok(resp) => {
                let delay = HTTP_INITIAL_RETRY_DELAY * (1u32 << (attempts - 1).min(31));
                tracing::debug!(
                    url = %request.url,
                    status = resp.status,
                    attempt = attempts,
                    max_retries = HTTP_MAX_RETRIES,
                    delay_ms = delay.as_millis(),
                    "Upstream returned 5xx, retrying"
                );
                tokio::time::sleep(delay).await;
            }
            // Transport error but retries exhausted.
            Err(e) if attempts > HTTP_MAX_RETRIES => return Err(e),
            // Transport error — backoff and retry.
            Err(e) => {
                let delay = HTTP_INITIAL_RETRY_DELAY * (1u32 << (attempts - 1).min(31));
                tracing::debug!(
                    url = %request.url,
                    error = %e,
                    attempt = attempts,
                    max_retries = HTTP_MAX_RETRIES,
                    delay_ms = delay.as_millis(),
                    "Upstream request failed, retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Send the HTTP request to the upstream AI provider.
async fn forward_request<H>(
    http_client: &H,
    request: &OutboundHttpRequest,
    headers: &[(String, String)],
) -> Result<OutboundHttpResponse, String>
where
    H: HttpClient,
{
    let resp: HttpResponse = http_client
        .send_request(&request.method, &request.url, headers, &request.body)
        .await?;

    Ok(OutboundHttpResponse {
        status: resp.status,
        headers: resp.headers,
        body: resp.body,
        error: None,
    })
}

/// Errors returned by the worker loop.
#[derive(Debug)]
pub enum WorkerError {
    JetStream(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JetStream(e) => write!(f, "JetStream error: {}", e),
        }
    }
}

impl std::error::Error for WorkerError {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    use crate::messages::OutboundHttpRequest;
    use crate::traits::HttpClient;

    use super::{
        HTTP_INITIAL_RETRY_DELAY, forward_request, forward_request_with_retry, process_request,
        resolve_token,
    };

    fn make_headers(auth: &str) -> Vec<(String, String)> {
        vec![("Authorization".to_string(), auth.to_string())]
    }

    fn make_request(url: &str, auth: &str, idempotency_key: &str) -> OutboundHttpRequest {
        OutboundHttpRequest {
            method: "POST".to_string(),
            url: url.to_string(),
            headers: make_headers(auth),
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: idempotency_key.to_string(),
        }
    }

    #[tokio::test]
    async fn resolve_token_success() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let headers = make_headers("Bearer tok_anthropic_prod_abc123");
        let key = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(key, "sk-ant-realkey");
    }

    #[tokio::test]
    async fn resolve_token_not_found() {
        let vault = MemoryVault::new();
        let headers = make_headers("Bearer tok_openai_prod_missing1");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn resolve_token_missing_header() {
        let vault = MemoryVault::new();
        let headers: Vec<(String, String)> = vec![];
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing Authorization"));
    }

    #[tokio::test]
    async fn resolve_token_not_bearer() {
        let vault = MemoryVault::new();
        let headers = make_headers("Basic dXNlcjpwYXNz");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a Bearer token"));
    }

    #[tokio::test]
    async fn resolve_token_real_key_passthrough() {
        let vault = MemoryVault::new();
        let headers = make_headers("Bearer sk-ant-realkey");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("does not look like a proxy token")
        );
    }

    #[tokio::test]
    async fn resolve_token_malformed_tok_missing_parts_is_rejected() {
        let vault = MemoryVault::new();

        let cases = ["tok_", "tok_provider", "tok_provider_env"];

        for bad_token in cases {
            let headers = make_headers(&format!("Bearer {bad_token}"));
            let result = resolve_token(&vault, &headers).await;
            assert!(result.is_err(), "token '{}' must be rejected", bad_token);
        }
    }

    #[tokio::test]
    async fn resolve_token_empty_key_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_openai_prod_emptykey1").unwrap();
        vault.store(&token, "").await.unwrap();

        let headers = make_headers("Bearer tok_openai_prod_emptykey1");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty key"));
    }

    // ── HttpClient tests using reqwest::Client against httpmock ──────────────

    #[tokio::test]
    async fn forward_request_success_with_real_http_client() {
        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method("POST").path("/v1/messages");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"msg_1"}"#);
            })
            .await;

        let client = reqwest::Client::new();
        let request = make_request(
            &format!("{}/v1/messages", server.base_url()),
            "Bearer tok_anthropic_prod_abc123",
            "idem-1",
        );
        let headers = vec![(
            "Authorization".to_string(),
            "Bearer sk-real-key".to_string(),
        )];
        let resp = forward_request(&client, &request, &headers).await.unwrap();
        assert_eq!(resp.status, 200);
    }

    /// Verify that the retry loop succeeds after an initial 5xx response.
    ///
    /// Uses `MockHttpClient` (feature-gated) rather than a real HTTP server so
    /// that the per-request response sequence is fully deterministic.
    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn forward_request_with_retry_succeeds_after_5xx() {
        use crate::mocks::MockHttpClient;

        let http = MockHttpClient::new();
        http.push_error(503, "service unavailable");
        http.push_ok(r#"{"ok":true}"#);

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_anthropic_prod_abc123",
            "idem-retry",
        );
        let headers = vec![(
            "Authorization".to_string(),
            "Bearer sk-real-key".to_string(),
        )];
        let resp = forward_request_with_retry(&http, &request, &headers)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn process_request_returns_401_when_token_not_found() {
        let server = httpmock::MockServer::start_async().await;
        let vault = MemoryVault::new(); // empty vault
        let client = reqwest::Client::new();

        let request = make_request(
            &format!("{}/v1/messages", server.base_url()),
            "Bearer tok_anthropic_prod_notfound",
            "idem-1",
        );

        let response = process_request(&request, &vault, &client).await;
        assert_eq!(response.status, 401);
        assert!(response.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn process_request_strips_real_key_from_response_headers() {
        let server = httpmock::MockServer::start_async().await;
        let real_key = "sk-ant-secretkey";
        server
            .mock_async(|when, then| {
                when.method("POST").path("/v1/messages");
                then.status(200).header("x-leaked-key", real_key).body("{}");
            })
            .await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, real_key).await.unwrap();

        let client = reqwest::Client::new();
        let request = make_request(
            &format!("{}/v1/messages", server.base_url()),
            "Bearer tok_anthropic_prod_abc123",
            "idem-1",
        );

        let response = process_request(&request, &vault, &client).await;
        assert_eq!(response.status, 200);
        for (_, v) in &response.headers {
            assert!(
                !v.contains(real_key),
                "response headers must not contain the real API key"
            );
        }
    }

    /// Verify that retry backoff calculation is deterministic.
    #[test]
    fn retry_backoff_doubles_each_attempt() {
        for attempt in 1u32..=4 {
            let delay = HTTP_INITIAL_RETRY_DELAY * (1u32 << (attempt - 1).min(31));
            let expected_ms = 100u64 * (1u64 << (attempt - 1));
            assert_eq!(
                delay.as_millis() as u64,
                expected_ms,
                "attempt {} delay mismatch",
                attempt
            );
        }
    }
}
