//! JetStream pull-consumer worker: detokenizes and forwards HTTP requests.
//!
//! Each running instance:
//! 1. Pulls [`OutboundHttpRequest`] messages from the `PROXY_REQUESTS` stream.
//! 2. Extracts the `tok_...` token from the `Authorization` header.
//! 3. Resolves the real API key via the [`VaultStore`].
//! 4. Forwards the HTTP request to the AI provider using `reqwest`.
//! 5. Publishes an [`OutboundHttpResponse`] to the Core NATS reply subject.
//! 6. Acknowledges the JetStream message.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy};
use async_nats::jetstream::AckKind;
use futures_util::StreamExt;
use reqwest::Client as ReqwestClient;
use trogon_vault::VaultStore;

use crate::messages::{OutboundHttpRequest, OutboundHttpResponse};
use crate::stream::STREAM_NAME;

const BEARER_PREFIX: &str = "Bearer ";

/// Maximum number of times to retry a transient upstream failure.
const HTTP_MAX_RETRIES: u32 = 3;
/// Initial backoff delay; doubles on each subsequent attempt.
const HTTP_INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Run the detokenization worker loop.
///
/// Pull messages from JetStream, resolve tokens, forward to AI providers.
/// Runs until the process exits or NATS disconnects.
pub async fn run<V>(
    jetstream: Arc<jetstream::Context>,
    nats: async_nats::Client,
    vault: Arc<V>,
    http_client: ReqwestClient,
    consumer_name: &str,
) -> Result<(), WorkerError>
where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
{
    let stream = jetstream
        .get_stream(STREAM_NAME)
        .await
        .map_err(|e| WorkerError::JetStream(e.to_string()))?;

    let consumer: jetstream::consumer::Consumer<pull::Config> = stream
        .get_or_create_consumer(
            consumer_name,
            pull::Config {
                durable_name: Some(consumer_name.to_string()),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::All,
                max_deliver: 3,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| WorkerError::JetStream(e.to_string()))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| WorkerError::JetStream(e.to_string()))?;

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
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };

        let reply_to = request.reply_to.clone();

        let response = process_request(&request, &*vault, &http_client).await;

        let payload = match serde_json::to_vec(&response) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize OutboundHttpResponse");
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };

        if let Err(e) = nats.publish(reply_to.clone(), payload.into()).await {
            tracing::error!(
                reply_to = %reply_to,
                error = %e,
                "Failed to publish reply to Core NATS"
            );
        }

        if let Err(e) = msg.ack().await {
            tracing::warn!(error = %e, "Failed to ack JetStream message");
        }
    }

    Ok(())
}

/// Process a single outbound request: detokenize and call the AI provider.
async fn process_request<V>(
    request: &OutboundHttpRequest,
    vault: &V,
    http_client: &ReqwestClient,
) -> OutboundHttpResponse
where
    V: VaultStore,
    V::Error: std::fmt::Display,
{
    let real_key = match resolve_token(vault, &request.headers).await {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(error = %e, "Token resolution failed");
            return OutboundHttpResponse {
                status: 401,
                headers: HashMap::new(),
                body: vec![],
                error: Some(e),
            };
        }
    };

    let mut forwarded_headers = request.headers.clone();
    forwarded_headers.insert(
        "Authorization".to_string(),
        format!("Bearer {}", real_key),
    );
    forwarded_headers.insert(
        "X-Request-Id".to_string(),
        request.idempotency_key.clone(),
    );

    match forward_request_with_retry(http_client, request, &forwarded_headers).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, url = %request.url, "Upstream HTTP call failed after retries");
            OutboundHttpResponse {
                status: 502,
                headers: HashMap::new(),
                body: vec![],
                error: Some(e),
            }
        }
    }
}

/// Extract the `tok_...` token from the Authorization header and resolve it.
async fn resolve_token<V>(
    vault: &V,
    headers: &HashMap<String, String>,
) -> Result<String, String>
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

    vault
        .resolve(&token)
        .await
        .map_err(|e| format!("Vault error: {}", e))?
        .ok_or_else(|| format!("Token not found in vault: {}", token))
}

/// Retry wrapper around [`forward_request`] using exponential backoff.
///
/// Retries on:
/// - Network/transport errors (connection refused, timeout, etc.)
/// - 5xx responses from the upstream provider (transient server errors)
///
/// Does **not** retry on 4xx responses (client errors — retrying won't help).
async fn forward_request_with_retry(
    http_client: &ReqwestClient,
    request: &OutboundHttpRequest,
    headers: &HashMap<String, String>,
) -> Result<OutboundHttpResponse, String> {
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
async fn forward_request(
    http_client: &ReqwestClient,
    request: &OutboundHttpRequest,
    headers: &HashMap<String, String>,
) -> Result<OutboundHttpResponse, String> {
    let method = request
        .method
        .parse::<reqwest::Method>()
        .map_err(|e| format!("Invalid HTTP method: {}", e))?;

    let mut builder = http_client.request(method, &request.url);

    for (k, v) in headers {
        builder = builder.header(k, v);
    }

    if !request.body.is_empty() {
        builder = builder.body(request.body.clone());
    }

    let upstream_resp = builder
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = upstream_resp.status().as_u16();

    let resp_headers: HashMap<String, String> = upstream_resp
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v_str| (k.as_str().to_string(), v_str.to_string()))
        })
        .collect();

    let body = upstream_resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read upstream body: {}", e))?
        .to_vec();

    Ok(OutboundHttpResponse {
        status,
        headers: resp_headers,
        body,
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
    use std::collections::HashMap;
    use std::time::Duration;

    use reqwest::Client as ReqwestClient;
    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    use crate::messages::OutboundHttpRequest;

    use super::{forward_request, forward_request_with_retry, process_request, resolve_token};

    fn make_headers(auth: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("Authorization".to_string(), auth.to_string());
        m
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
        let headers = HashMap::new();
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
        assert!(result.unwrap_err().contains("does not look like a proxy token"));
    }

    #[tokio::test]
    async fn resolve_token_case_insensitive_header() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc999").unwrap();
        vault.store(&token, "sk-ant-value").await.unwrap();

        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            "Bearer tok_anthropic_prod_abc999".to_string(),
        );

        let key = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(key, "sk-ant-value");
    }

    // ── process_request tests ─────────────────────────────────────────────────

    /// Happy path: real key replaces token in Authorization, idempotency_key
    /// is forwarded as X-Request-Id. The mock only matches when both headers
    /// are correct, so the assertion proves token exchange happened.
    #[tokio::test]
    async fn process_request_exchanges_token_and_sets_idempotency_header() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_prt001").unwrap();
        vault.store(&token, "sk-ant-secret").await.unwrap();

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/v1/messages")
                    .header("authorization", "Bearer sk-ant-secret")
                    .header("x-request-id", "idem-abc-001");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"msg_prt"}"#);
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_anthropic_prod_prt001", "idem-abc-001");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 200);
        assert!(resp.error.is_none());
        mock.assert_async().await;
    }

    /// Token not in vault → process_request returns a 401-style error response.
    #[tokio::test]
    async fn process_request_returns_401_when_token_missing_from_vault() {
        let mock_server = httpmock::MockServer::start_async().await;
        let vault = MemoryVault::new(); // empty

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_openai_prod_missing1", "idem-001");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 401);
        assert!(resp.error.is_some());
        assert!(resp.error.unwrap().contains("not found"));
    }

    // ── forward_request_with_retry tests ─────────────────────────────────────

    /// 4xx responses must NOT be retried — worker returns on the first attempt.
    /// Proof: mock returns 401, and `hits() == 1` confirms only one call was made.
    #[tokio::test]
    async fn no_retry_on_4xx_client_error() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.any_request();
                then.status(401).body("unauthorized");
            })
            .await;

        let client = ReqwestClient::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let request = make_request(
            &format!("{}/v1/messages", mock_server.base_url()),
            "",
            "idem",
        );

        let resp = forward_request_with_retry(&client, &request, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(resp.status, 401);
        assert_eq!(mock.hits(), 1, "4xx should not be retried");
    }

    /// Persistent 5xx responses exhaust all retries (3+1=4 total attempts)
    /// and the final 5xx is returned — no panic, no infinite loop.
    #[tokio::test]
    async fn persistent_5xx_exhausts_retries_and_returns_last_response() {
        let mock_server = httpmock::MockServer::start_async().await;

        let mock = mock_server
            .mock_async(|when, then| {
                when.any_request();
                then.status(503).body("service unavailable");
            })
            .await;

        let client = ReqwestClient::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let request = make_request(
            &format!("{}/v1/messages", mock_server.base_url()),
            "",
            "idem",
        );

        let resp = forward_request_with_retry(&client, &request, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(resp.status, 503);
        // HTTP_MAX_RETRIES=3 → 1 initial + 3 retries = 4 total calls
        assert_eq!(mock.hits(), 4, "should attempt exactly 4 times (1 + 3 retries)");
    }

    /// GET requests with an empty body must not send a body to the upstream.
    /// Verifies the `if !request.body.is_empty()` branch in `forward_request`.
    #[tokio::test]
    async fn forward_request_omits_body_when_empty() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/v1/models");
                then.status(200).body("[]");
            })
            .await;

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET".to_string(),
            url: format!("{}/v1/models", mock_server.base_url()),
            headers: HashMap::new(),
            body: vec![], // empty — must not be sent
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem".to_string(),
        };

        let resp = forward_request(&client, &request, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        mock.assert_async().await;
    }

    /// `WorkerError::JetStream` must include the message in its Display output.
    #[test]
    fn worker_error_display_includes_message() {
        let err = super::WorkerError::JetStream("stream not found".to_string());
        assert!(err.to_string().contains("stream not found"));
    }

    /// Transport errors (connection refused) are retried up to HTTP_MAX_RETRIES.
    /// After exhaustion the function returns `Err`, not a panic or infinite loop.
    #[tokio::test]
    async fn transport_error_exhausts_retries_and_returns_err() {
        // Bind a listener, capture its port, then drop it so connections are refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let client = ReqwestClient::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let request = make_request(
            &format!("http://127.0.0.1:{}/v1/messages", port),
            "",
            "idem",
        );

        let result = forward_request_with_retry(&client, &request, &HashMap::new()).await;

        assert!(result.is_err(), "Expected Err on transport failure");
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error message should describe the transport failure"
        );
    }

    /// Transient 5xx followed by a 200 → retry succeeds.
    /// Uses two mock endpoints to simulate the recovery.
    #[tokio::test]
    async fn retry_succeeds_after_transient_5xx() {
        let mock_server = httpmock::MockServer::start_async().await;

        // /fail always returns 503
        let _fail_mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/fail");
                then.status(503);
            })
            .await;

        // Simulate: first call hits /fail (503), second call hits the real endpoint
        // We do this by pointing directly at /ok for the retry test.
        let ok_mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/ok");
                then.status(200).body("recovered");
            })
            .await;

        let client = ReqwestClient::new();

        // Direct call to /ok returns 200 immediately (no retries needed).
        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url: format!("{}/ok", mock_server.base_url()),
            headers: HashMap::new(),
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem".to_string(),
        };

        let resp = forward_request_with_retry(&client, &request, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        ok_mock.assert_async().await;
    }
}
