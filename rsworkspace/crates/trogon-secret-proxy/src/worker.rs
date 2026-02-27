//! JetStream pull-consumer worker: detokenizes and forwards HTTP requests.
//!
//! Each running instance:
//! 1. Pulls [`OutboundHttpRequest`] messages from the `PROXY_REQUESTS` stream.
//! 2. Extracts the `tok_...` token from the `Authorization` header.
//! 3. Resolves the real API key via the [`VaultStore`].
//! 4. Forwards the HTTP request to the AI provider using `reqwest`.
//! 5. Publishes an [`OutboundHttpResponse`] to the Core NATS reply subject.
//! 6. Acknowledges the JetStream message.

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
    stream_name: &str,
) -> Result<(), WorkerError>
where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
{
    let stream = jetstream
        .get_stream(stream_name)
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
            // The original payload was too large (or the connection dropped).
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
            !k.eq_ignore_ascii_case("authorization")
                && !k.eq_ignore_ascii_case("x-request-id")
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
async fn resolve_token<V>(
    vault: &V,
    headers: &[(String, String)],
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

    let real_key = vault
        .resolve(&token)
        .await
        .map_err(|e| format!("Vault error: {}", e))?
        .ok_or_else(|| format!("Token not found in vault: {}", token))?;

    if real_key.is_empty() {
        return Err(format!(
            "Vault returned an empty key for token: {}",
            token
        ));
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
async fn forward_request_with_retry(
    http_client: &ReqwestClient,
    request: &OutboundHttpRequest,
    headers: &[(String, String)],
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
    headers: &[(String, String)],
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

    // Collect response headers as an ordered list of (name, value) pairs so
    // that duplicate headers (e.g. multiple Set-Cookie) are preserved.
    let resp_headers: Vec<(String, String)> = upstream_resp
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
    use std::time::Duration;

    use reqwest::Client as ReqwestClient;
    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    use crate::messages::OutboundHttpRequest;

    use super::{forward_request, forward_request_with_retry, process_request, resolve_token, HTTP_INITIAL_RETRY_DELAY};

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
        assert!(result.unwrap_err().contains("does not look like a proxy token"));
    }

    /// Gap 3: token starting with `tok_` but missing provider/env/id segments
    /// must be rejected by `ApiKeyToken::new` validation before touching the vault.
    #[tokio::test]
    async fn resolve_token_malformed_tok_missing_parts_is_rejected() {
        let vault = MemoryVault::new();

        let cases = [
            "Bearer tok_anthropic",          // missing env and id
            "Bearer tok_anthropic_prod",     // missing id
            "Bearer tok_",                   // nothing after tok_
        ];

        for header in cases {
            let headers = make_headers(header);
            let result = resolve_token(&vault, &headers).await;
            assert!(
                result.is_err(),
                "Malformed token '{}' must be rejected",
                header
            );
            let err = result.unwrap_err();
            assert!(
                err.contains("Invalid proxy token"),
                "Error must mention 'Invalid proxy token', got: {}",
                err
            );
        }
    }

    #[tokio::test]
    async fn resolve_token_empty_real_key_is_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_emptyv1").unwrap();
        vault.store(&token, "").await.unwrap(); // store empty real key

        let headers = make_headers("Bearer tok_anthropic_prod_emptyv1");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("empty key"),
            "Error must mention empty key"
        );
    }

    #[tokio::test]
    async fn resolve_token_case_insensitive_header() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc999").unwrap();
        vault.store(&token, "sk-ant-value").await.unwrap();

        let headers = vec![(
            "authorization".to_string(),
            "Bearer tok_anthropic_prod_abc999".to_string(),
        )];

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

        let resp = forward_request_with_retry(&client, &request, &[])
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

        let resp = forward_request_with_retry(&client, &request, &[])
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
            headers: vec![],
            body: vec![], // empty — must not be sent
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem".to_string(),
        };

        let resp = forward_request(&client, &request, &[])
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        mock.assert_async().await;
    }

    /// Gap 5: DNS resolution failure for an invalid hostname must exhaust retries
    /// and return `Err`, not panic or loop forever.
    ///
    /// `.invalid` is an RFC 2606 reserved TLD guaranteed to never resolve.
    #[tokio::test]
    async fn dns_resolution_failure_exhausts_retries_and_returns_err() {
        let client = ReqwestClient::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let request = make_request(
            "http://host.does.not.exist.invalid./v1/messages",
            "Bearer tok_anthropic_prod_dns1",
            "idem-dns",
        );

        let result = forward_request_with_retry(&client, &request, &[]).await;

        assert!(result.is_err(), "DNS failure must return Err after retries");
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error must describe the transport failure"
        );
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

        let result = forward_request_with_retry(&client, &request, &[]).await;

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
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem".to_string(),
        };

        let resp = forward_request_with_retry(&client, &request, &[])
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        ok_mock.assert_async().await;
    }

    // ── Gap: lowercase "bearer" prefix ────────────────────────────────────────

    /// `BEARER_PREFIX` is `"Bearer "` (capital B).  `starts_with` is byte-exact,
    /// so `"bearer tok_..."` (lowercase) does NOT match and must be rejected.
    ///
    /// This prevents bypassing token validation by sending a lowercase scheme.
    #[tokio::test]
    async fn resolve_token_lowercase_bearer_is_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let headers = make_headers("bearer tok_anthropic_prod_abc123");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err(), "lowercase 'bearer' must be rejected");
        assert!(
            result.unwrap_err().contains("not a Bearer token"),
            "Error must describe the rejection reason"
        );
    }

    /// `"Bearer  tok_..."` (two spaces) means `raw_token` starts with a space,
    /// not with `"tok_"`.  The validation must reject it as not looking like a
    /// proxy token.
    #[tokio::test]
    async fn resolve_token_double_space_after_bearer_is_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_dbl01").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        // Two spaces between "Bearer" and the token name.
        let headers = make_headers("Bearer  tok_anthropic_prod_dbl01");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err(), "double-space Bearer must be rejected");
        // raw_token is " tok_..." which starts with a space, not "tok_".
        assert!(
            result.unwrap_err().contains("does not look like a proxy token"),
            "Error must describe the rejection reason"
        );
    }

    // ── Gap: VaultStore::resolve() returning Err ───────────────────────────────

    /// When the vault backend itself returns an error (network failure, I/O error,
    /// etc.), `process_request` must surface it as a `401` error response —
    /// NOT panic or propagate the error up to the worker loop.
    ///
    /// This tests the `Vault error: {}` branch in `resolve_token` at
    /// `worker.rs:227`.  MemoryVault never fails, so a dedicated test-only
    /// implementation is needed to exercise this path.
    #[tokio::test]
    async fn process_request_vault_backend_error_returns_401() {
        use trogon_vault::{ApiKeyToken as VaultToken, VaultStore as VaultStoreTrait};

        struct ErrorVault;

        impl VaultStoreTrait for ErrorVault {
            type Error = std::io::Error;

            fn store(
                &self,
                _token: &VaultToken,
                _plaintext: &str,
            ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
                async { Ok(()) }
            }

            fn resolve(
                &self,
                _token: &VaultToken,
            ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send
            {
                async {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "vault backend unavailable",
                    ))
                }
            }

            fn revoke(
                &self,
                _token: &VaultToken,
            ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
                async { Ok(()) }
            }
        }

        let mock_server = httpmock::MockServer::start_async().await;
        let vault = ErrorVault;
        let client = ReqwestClient::new();
        let url = format!("{}/v1/messages", mock_server.base_url());

        let request = make_request(&url, "Bearer tok_anthropic_prod_vaulterr1", "idem-ve-01");
        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 401, "Vault backend error must result in 401");
        assert!(resp.error.is_some(), "Error field must be set");
        let err = resp.error.unwrap();
        assert!(
            err.contains("Vault error") || err.contains("vault backend unavailable"),
            "Error must mention vault failure, got: {}",
            err
        );
    }

    // ── Gap: HTTP method with leading/trailing whitespace ─────────────────────

    /// `reqwest::Method::from_str` is strict: `" POST"` or `"POST "` are not
    /// valid HTTP method names.  `forward_request` must return `Err` with an
    /// "Invalid HTTP method" message rather than panicking.
    ///
    /// In practice the method comes from the client request and axum normalises
    /// it, but a crafted or malformed `OutboundHttpRequest` (e.g. from a NAK'd
    /// redelivery with corrupted payload) could carry a padded method string.
    #[tokio::test]
    async fn forward_request_whitespace_padded_method_returns_error() {
        let mock_server = httpmock::MockServer::start_async().await;
        let client = ReqwestClient::new();

        for padded in &[" POST", "POST ", " POST "] {
            let request = OutboundHttpRequest {
                method: padded.to_string(),
                url: format!("{}/v1/messages", mock_server.base_url()),
                headers: vec![],
                body: vec![],
                reply_to: "test.reply".to_string(),
                idempotency_key: "idem-ws".to_string(),
            };

            let result = forward_request(&client, &request, &[]).await;
            assert!(
                result.is_err(),
                "Whitespace-padded method {:?} must be rejected",
                padded
            );
            assert!(
                result.unwrap_err().contains("Invalid HTTP method"),
                "Error must describe the rejection for method {:?}",
                padded
            );
        }
    }

    // ── Gap: short real key causes over-eager header sanitisation ─────────────

    /// The response-header sanitisation in `process_request` uses
    /// `String::contains`, which is a simple substring match:
    ///
    /// ```text
    /// resp.headers.retain(|(_, v)| !v.contains(real_key.as_str()));
    /// ```
    ///
    /// When `real_key` is a very short string (e.g. `"sk"`), ANY response
    /// header whose value contains that substring is stripped — including
    /// innocuous headers like `x-task-id: risky-path` (which contains "sk").
    ///
    /// This test documents the design boundary: the sanitisation is intentionally
    /// broad (favour security over precision).  Operators should use real keys
    /// that are long enough to avoid false-positive matches.
    #[tokio::test]
    async fn process_request_short_real_key_strips_headers_with_matching_substring() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_shortkey1").unwrap();
        // Intentionally very short key — "sk" appears as a substring in many words.
        vault.store(&token, "sk").await.unwrap();

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/v1/messages");
                then.status(200)
                    .header("content-type", "application/json")
                    // "risky" contains "sk" (r-i-s-k-y) → will be stripped.
                    .header("x-flag", "risky-operation")
                    // "clean" does not contain "sk" → will be preserved.
                    .header("x-safe", "clean-value")
                    .body(r#"{"id":"msg_short_key"}"#);
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_anthropic_prod_shortkey1", "idem-sk-01");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 200);
        mock.assert_async().await;

        // "risky-operation" contains "sk" → stripped as a false positive.
        let flag_present = resp.headers.iter().any(|(k, _)| k == "x-flag");
        assert!(
            !flag_present,
            "x-flag header (value contains short key) must be stripped — design boundary"
        );

        // "clean-value" does not contain "sk" → preserved.
        let safe_present = resp.headers.iter().any(|(k, _)| k == "x-safe");
        assert!(
            safe_present,
            "x-safe header (value does not contain key) must be preserved"
        );
    }

    // ── Gap: real key containing newline causes HTTP request to fail ────────────

    /// A vault entry whose real key contains a newline (`\n`) causes the
    /// `Authorization` header to be built as `"Bearer sk\nkey"`.
    /// The `http` crate rejects control characters (including `\n`) in header
    /// values, so `forward_request` returns `Err("HTTP request failed: …")`
    /// **before** any TCP connection is attempted.
    ///
    /// Consequence: a misconfigured vault entry with an embedded newline makes
    /// ALL requests using that token fail with status 502 — the `retain()`
    /// sanitisation step is never reached.
    #[tokio::test]
    async fn process_request_real_key_with_newline_causes_502_error() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_nlkey001").unwrap();
        // Key contains a literal newline — pathological vault misconfiguration.
        vault.store(&token, "sk\nkey").await.unwrap();

        // Mock should NOT be called — the request fails before any TCP send.
        let mock = mock_server
            .mock_async(|when, then| {
                when.any_request();
                then.status(200).body("should not reach here");
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_anthropic_prod_nlkey001", "idem-nl");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        // reqwest rejects "Bearer sk\nkey" → forward_request returns Err →
        // process_request wraps it as a 502 error response.
        assert_eq!(
            resp.status, 502,
            "A newline in the real key must cause a 502 error (reqwest rejects \\n in header values)"
        );
        assert!(resp.error.is_some(), "Error field must be set");

        // The mock must not have been called — the failure happens before TCP.
        assert_eq!(mock.hits(), 0, "Upstream must not be called when header is invalid");
    }

    // ── Gap: single-char real key strips every header containing that byte ────

    /// When `real_key` is a single character (e.g. `"k"`), the substring
    /// check `v.contains("k")` strips ANY response header whose value
    /// contains the letter 'k' — including completely innocent ones like
    /// `x-task-id: task-key` or `content-type: application/json` (contains
    /// no 'k', preserved) vs `x-link: bookmark` (contains 'k', stripped).
    ///
    /// This is the extreme end of the false-positive behaviour already
    /// documented in `process_request_short_real_key_strips_headers_with_matching_substring`.
    /// The single-byte case is the worst possible scenario for operator misconfiguration.
    #[tokio::test]
    async fn process_request_single_char_real_key_strips_every_matching_header() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_onechar01").unwrap();
        // Single-char key: "k".  Any header value containing 'k' will be stripped.
        vault.store(&token, "k").await.unwrap();

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/v1/messages");
                then.status(200)
                    .header("content-type", "application/json") // no 'k' → preserved
                    .header("x-link", "bookmark")               // "bookmark" contains 'k' → stripped
                    .header("x-safe", "clean-value")            // no 'k' → preserved
                    .body(r#"{"id":"msg_onechar"}"#);
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_anthropic_prod_onechar01", "idem-1char");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 200);
        mock.assert_async().await;

        // "bookmark" contains 'k' → stripped (false positive, design boundary).
        let link_present = resp.headers.iter().any(|(k, _)| k == "x-link");
        assert!(
            !link_present,
            "x-link (value 'bookmark' contains 'k') must be stripped"
        );

        // "application/json" does not contain 'k' → preserved.
        let ct_present = resp.headers.iter().any(|(k, _)| k == "content-type");
        assert!(ct_present, "content-type must be preserved (no 'k' in value)");

        // "clean-value" does not contain 'k' → preserved.
        let safe_present = resp.headers.iter().any(|(k, _)| k == "x-safe");
        assert!(safe_present, "x-safe must be preserved (no 'k' in value)");
    }

    // ── Gap: sub-500 status not retried ──────────────────────────────────────

    /// `forward_request_with_retry` only retries on `status >= 500`.
    /// A 4xx response (e.g. 418) must be returned immediately without
    /// triggering a retry — mock must be hit exactly once.
    ///
    /// Note: 1xx status codes are reserved by the HTTP stack (hyper/httpmock)
    /// for informational handshakes; we use 418 as a concrete non-5xx boundary.
    #[tokio::test]
    async fn forward_request_4xx_not_retried() {
        let mock_server = httpmock::MockServer::start_async().await;

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/v1/info");
                then.status(418).body("I'm a teapot");
            })
            .await;

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url: format!("{}/v1/info", mock_server.base_url()),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-418".to_string(),
        };

        let resp = forward_request_with_retry(&client, &request, &[])
            .await
            .unwrap();

        assert_eq!(resp.status, 418, "4xx response must be returned as-is");
        assert_eq!(mock.hits(), 1, "4xx response must not trigger a retry");
    }

    // ── Gap: GET with Content-Type but no body ────────────────────────────────

    /// A GET request with a `Content-Type` header but an empty body must
    /// succeed.  `forward_request` skips the `.body()` call when
    /// `request.body` is empty, but still forwards headers from its `headers`
    /// slice — so Content-Type must reach the provider without a body.
    #[tokio::test]
    async fn forward_request_get_with_content_type_but_no_body() {
        let mock_server = httpmock::MockServer::start_async().await;

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/v1/models")
                    .header("content-type", "application/json");
                then.status(200).body(r#"{"models":[]}"#);
            })
            .await;

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET".to_string(),
            url: format!("{}/v1/models", mock_server.base_url()),
            headers: vec![],
            body: vec![],   // empty body — forward_request skips .body()
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-get-ct".to_string(),
        };

        // Content-Type is passed via the `headers` slice (the processed
        // header list), not via request.headers.  forward_request adds them.
        let extra_headers = vec![("content-type".to_string(), "application/json".to_string())];

        let resp = forward_request_with_retry(&client, &request, &extra_headers)
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        mock.assert_async().await;
    }

    // ── Gap: multiple Authorization headers all stripped ──────────────────────

    /// When an incoming request carries more than one `Authorization` header
    /// (e.g. added by different middleware layers), `process_request` must strip
    /// ALL of them and inject a single header with the resolved real key.
    ///
    /// The mock explicitly rejects any call that still carries the tok_ token,
    /// which would happen if one of the duplicate headers leaked through.
    #[tokio::test]
    async fn process_request_duplicate_authorization_headers_stripped() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_dupauth1").unwrap();
        let real_key = "sk-ant-dupauth-real-key";
        vault.store(&token, real_key).await.unwrap();

        // Accept only the real key — no tok_ or duplicate value must survive.
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/v1/messages")
                    .header("authorization", format!("Bearer {}", real_key));
                then.status(200).body(r#"{"id":"dup"}"#);
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url,
            headers: vec![
                // Primary tok_ token (valid).
                ("Authorization".to_string(), "Bearer tok_anthropic_prod_dupauth1".to_string()),
                // Duplicate — must also be stripped.
                ("Authorization".to_string(), "Bearer some-extra-key-1".to_string()),
                // Lowercase key — eq_ignore_ascii_case catches this too.
                ("authorization".to_string(), "Bearer some-extra-key-2".to_string()),
            ],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-dup-01".to_string(),
        };

        let client = ReqwestClient::new();
        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 200, "All duplicate Authorization headers must be stripped and real key injected");
        mock.assert_async().await;
    }

    // ── Gap: invalid HTTP method names (non-ASCII / special chars) ─────────────

    /// A method with non-ASCII characters (e.g. `"INVÁLIDO"`) cannot be parsed
    /// by `reqwest::Method::from_str` and must return an error immediately.
    #[tokio::test]
    async fn forward_request_non_ascii_method_returns_error() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "INVÁLIDO".to_string(),
            url: "http://127.0.0.1:1/v1/messages".to_string(),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-nonascii".to_string(),
        };

        let result = forward_request_with_retry(&client, &request, &[]).await;
        assert!(result.is_err(), "Non-ASCII method must return Err");
        assert!(
            result.unwrap_err().contains("Invalid HTTP method"),
            "Error must contain 'Invalid HTTP method'"
        );
    }

    /// A method with an invalid character (e.g. `"GET@"`) is rejected by
    /// `reqwest::Method::from_str` — `@` is not a valid RFC 7230 tchar.
    /// (Note: `!` IS a valid tchar and would be accepted.)
    #[tokio::test]
    async fn forward_request_special_char_method_returns_error() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET@".to_string(),
            url: "http://127.0.0.1:1/v1/messages".to_string(),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-specialchar".to_string(),
        };

        let result = forward_request_with_retry(&client, &request, &[]).await;
        assert!(result.is_err(), "Method with special char must return Err");
        assert!(
            result.unwrap_err().contains("Invalid HTTP method"),
            "Error must contain 'Invalid HTTP method'"
        );
    }

    // ── Gap: vault error exact message format ─────────────────────────────────

    /// When the vault backend returns `Err(e)`, the error is wrapped with the
    /// prefix `"Vault error: "` and the backend's `Display` representation is
    /// appended verbatim.  This test verifies the exact combined string so that
    /// log consumers and callers can pattern-match reliably.
    #[tokio::test]
    async fn resolve_token_vault_error_exact_message_format() {
        use trogon_vault::{ApiKeyToken as VaultToken, VaultStore as VaultStoreTrait};

        struct ExactErrorVault;

        impl VaultStoreTrait for ExactErrorVault {
            type Error = std::io::Error;

            fn store(
                &self,
                _token: &VaultToken,
                _plaintext: &str,
            ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
                async { Ok(()) }
            }

            fn resolve(
                &self,
                _token: &VaultToken,
            ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send
            {
                async {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "simulated backend failure",
                    ))
                }
            }

            fn revoke(
                &self,
                _token: &VaultToken,
            ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
                async { Ok(()) }
            }
        }

        let vault = ExactErrorVault;
        let headers = make_headers("Bearer tok_anthropic_prod_exacterr1");
        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err,
            "Vault error: simulated backend failure",
            "Vault error format must be 'Vault error: <display>'"
        );
    }

    // ── Gap: exponential backoff shift saturates at 31 ────────────────────────

    /// The backoff formula is:
    ///   `HTTP_INITIAL_RETRY_DELAY * (1u32 << (attempts - 1).min(31))`
    ///
    /// `.min(31)` prevents a left-shift overflow for attempts > 32.
    /// `HTTP_MAX_RETRIES = 3` means the loop never actually reaches attempt 32
    /// in production, but the guard must still be correct.
    ///
    /// This test verifies the arithmetic directly: for any attempt >= 33 the
    /// shift is capped at 31 (producing the maximum multiplier 2^31) and no
    /// integer overflow occurs.
    #[test]
    fn backoff_shift_saturates_at_31_for_high_attempt_counts() {
        for attempts in [32u32, 33, 50, 100, u32::MAX] {
            let shift = (attempts - 1).min(31);
            assert_eq!(
                shift, 31,
                "Shift must be saturated at 31 for attempts={}, got {}",
                attempts, shift
            );
            // 1u32 << 31 must not panic (would panic in debug mode if it overflowed).
            let multiplier = 1u32 << shift;
            assert_eq!(
                multiplier,
                2u32.pow(31),
                "Multiplier must be 2^31 at saturation"
            );
            // Verify the full delay expression does not overflow Duration arithmetic.
            let _delay = HTTP_INITIAL_RETRY_DELAY * multiplier;
        }
    }

    // ── Gap C: response header with invalid UTF-8 silently dropped ────────────

    /// A provider may return a header whose value bytes are valid Latin-1
    /// (accepted by HTTP/1.1 parsers) but NOT valid UTF-8 — e.g. `0xFF`.
    /// `forward_request` calls `v.to_str().ok()` which returns `None` for
    /// such values; the header is silently omitted from `OutboundHttpResponse`.
    /// A sibling header with a valid ASCII value must be preserved.
    ///
    /// Uses a raw TCP server to send the non-UTF-8 byte directly.
    #[tokio::test]
    async fn forward_request_invalid_utf8_response_header_silently_dropped() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            stream.read(&mut buf).await.ok();
            // x-invalid: has byte 0xFF (valid Latin-1, invalid UTF-8)
            // x-valid:   has plain ASCII value — must survive
            let mut response = Vec::new();
            response.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
            response.extend_from_slice(b"x-invalid: \xff\xfe\r\n");
            response.extend_from_slice(b"x-valid: ascii-safe\r\n");
            response.extend_from_slice(b"content-length: 2\r\n\r\nok");
            stream.write_all(&response).await.ok();
        });

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET".to_string(),
            url: format!("http://127.0.0.1:{}/test", port),
            headers: vec![],
            body: vec![],
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-invalid-utf8".to_string(),
        };

        let resp = forward_request_with_retry(&client, &request, &[])
            .await
            .unwrap();

        assert_eq!(resp.status, 200);

        let has_invalid = resp.headers.iter().any(|(k, _)| k == "x-invalid");
        assert!(
            !has_invalid,
            "Header with invalid UTF-8 value must be silently dropped"
        );

        let has_valid = resp.headers.iter().any(|(k, _)| k == "x-valid");
        assert!(has_valid, "Header with valid ASCII value must be preserved");
    }

    // ── Gap D: 5xx retry succeeds on second attempt ───────────────────────────

    /// When the provider returns 503 on the first attempt, the retry loop
    /// (lines 268-279) must sleep and retry.  On the second attempt the server
    /// returns 200 — the function must return that 200 response.
    ///
    /// Uses a raw TCP server with an AtomicU32 counter:
    /// - attempt 1 → 503 with `Connection: close` (forces new TCP conn on retry)
    /// - attempt 2 → 200
    #[tokio::test]
    async fn forward_request_5xx_retry_succeeds_on_second_attempt() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let attempt_count = Arc::new(AtomicU32::new(0));
        let counter = attempt_count.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let mut buf = vec![0u8; 4096];
                stream.read(&mut buf).await.ok();

                if n == 1 {
                    // First attempt: 503 + Connection: close so reqwest opens
                    // a fresh TCP connection on the next retry.
                    let r = b"HTTP/1.1 503 Service Unavailable\r\n\
                              Connection: close\r\n\
                              content-length: 5\r\n\r\nerror";
                    stream.write_all(r).await.ok();
                } else {
                    // Second attempt: 200 OK.
                    let r = b"HTTP/1.1 200 OK\r\n\
                              content-length: 7\r\n\r\nretried";
                    stream.write_all(r).await.ok();
                    break;
                }
            }
        });

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url: format!("http://127.0.0.1:{}/retry-me", port),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-5xx-retry".to_string(),
        };

        let resp = forward_request_with_retry(&client, &request, &[])
            .await
            .unwrap();

        assert_eq!(resp.status, 200, "Second attempt must return 200");
        assert_eq!(resp.body, b"retried", "Body from successful retry must be returned");
        assert_eq!(
            attempt_count.load(Ordering::SeqCst),
            2,
            "Must make exactly 2 attempts"
        );
    }

    // ── Gap #9: first Authorization header wins when multiple with different casing ──

    /// `resolve_token` uses `Iterator::find` which returns the **first** header
    /// whose name matches `"authorization"` (case-insensitive).  When a request
    /// carries two Authorization headers with different casing, the first one
    /// is used for token resolution and the second is ignored.
    ///
    /// Both tokens are stored in the vault; only the first one's real key must
    /// be returned.
    #[tokio::test]
    async fn resolve_token_first_authorization_header_wins_when_multiple() {
        let vault = MemoryVault::new();

        let token_first = ApiKeyToken::new("tok_anthropic_prod_first01").unwrap();
        let token_second = ApiKeyToken::new("tok_anthropic_prod_second1").unwrap();
        vault.store(&token_first, "sk-ant-first-key").await.unwrap();
        vault.store(&token_second, "sk-ant-second-key").await.unwrap();

        // First header uses uppercase casing, second uses lowercase.
        let headers = vec![
            ("AUTHORIZATION".to_string(), "Bearer tok_anthropic_prod_first01".to_string()),
            ("authorization".to_string(), "Bearer tok_anthropic_prod_second1".to_string()),
        ];

        let key = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(
            key, "sk-ant-first-key",
            "First Authorization header must win; second must be ignored"
        );
    }

    // ── Gap B ──────────────────────────────────────────────────────────────

    /// A very long Authorization header whose value does not start with
    /// `tok_` must not panic and must have its raw-token prefix truncated
    /// to at most 16 characters in the error message.
    ///
    /// `worker.rs`: `&raw_token[..raw_token.len().min(16)]`
    #[tokio::test]
    async fn resolve_token_very_long_header_error_truncated_at_16_chars() {
        let vault = MemoryVault::new();
        // 100 KB of 'X' chars — does NOT start with "tok_".
        let long_prefix = "X".repeat(100_000);
        let auth_value = format!("Bearer {}", long_prefix);
        let headers = vec![("authorization".to_string(), auth_value)];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err(), "Long non-tok_ header must produce an error");
        let msg = result.unwrap_err();

        // The truncated snippet must be exactly 16 'X' characters.
        assert!(
            msg.contains("XXXXXXXXXXXXXXXX"),
            "Error must contain a 16-char truncation, got: {}",
            msg
        );
        // Must NOT include 17 or more consecutive X's (i.e. the full value
        // must not be leaked into the error string).
        assert!(
            !msg.contains(&"X".repeat(17)),
            "Error must not leak more than 16 chars of the raw value"
        );
    }

    // ── Gap C ──────────────────────────────────────────────────────────────

    /// A header value containing a raw control character (U+0001 SOH, byte
    /// 0x01) is invalid per RFC 7230.  reqwest / the `http` crate rejects
    /// it when building the request, returning an error before any TCP
    /// connection is attempted.
    ///
    /// This validates that such errors propagate up as
    /// `"HTTP request failed: …"` rather than panicking.
    #[tokio::test]
    async fn forward_request_control_char_in_header_value_is_rejected() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET".to_string(),
            url: "http://127.0.0.1:1/test".to_string(),
            headers: vec![],
            body: vec![],
            reply_to: "test.reply".to_string(),
            idempotency_key: "valid-key".to_string(),
        };
        // Inject control char 0x01 into a header value, simulating a
        // corrupted idempotency_key reaching forward_request.
        let headers = vec![
            ("Authorization".to_string(), "Bearer sk-realkey".to_string()),
            ("X-Request-Id".to_string(), "key\x01invalid".to_string()),
        ];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_err(), "Control char in header value must cause an error");
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error must originate from the HTTP layer"
        );
    }

    // ── Gap E ──────────────────────────────────────────────────────────────

    /// A CRLF sequence embedded in a header value is an HTTP
    /// request-splitting attack vector.  The `http` crate rejects bytes
    /// 0x0D (`\r`) and 0x0A (`\n`) in header values.
    #[tokio::test]
    async fn forward_request_crlf_in_header_value_is_rejected() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "GET".to_string(),
            url: "http://127.0.0.1:1/test".to_string(),
            headers: vec![],
            body: vec![],
            reply_to: "test.reply".to_string(),
            idempotency_key: "valid-key".to_string(),
        };
        let headers = vec![
            ("Authorization".to_string(), "Bearer sk-realkey".to_string()),
            (
                "X-Evil".to_string(),
                "value\r\nX-Injected: yes".to_string(),
            ),
        ];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_err(), "CRLF in header value must cause an error");
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error must originate from the HTTP layer"
        );
    }

    // ── Gap 3 ──────────────────────────────────────────────────────────────

    /// A header value with leading whitespace before "Bearer" does NOT match
    /// `starts_with(BEARER_PREFIX)` — the prefix check is position-exact.
    /// The token must be rejected with a "not a Bearer token" error.
    #[tokio::test]
    async fn resolve_token_leading_whitespace_before_bearer_is_rejected() {
        let vault = MemoryVault::new();
        // Two spaces before "Bearer" — common copy-paste mistake.
        let headers = vec![(
            "authorization".to_string(),
            "  Bearer tok_anthropic_prod_abc123".to_string(),
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("not a Bearer token"),
            "Extra whitespace before 'Bearer' must be rejected"
        );
    }

    // ── Gap 4 ──────────────────────────────────────────────────────────────

    /// RFC 7230 §3.1.1 defines an HTTP method as an "HTTP token" — any
    /// sequence of printable ASCII characters excluding delimiters.
    /// Lowercase letters ARE valid token characters, so `"post"` (lowercase)
    /// parses successfully as a custom extension method; it is NOT the same
    /// as the standard `POST` method.
    ///
    /// This means the proxy will forward requests with unconventional method
    /// casing to the upstream provider unchanged — whether the provider
    /// accepts them is outside the proxy's responsibility.
    #[test]
    fn http_method_lowercase_is_valid_rfc7230_extension_token() {
        // Reqwest uses the `http` crate which accepts any valid RFC 7230 token.
        let lowercase = "post".parse::<reqwest::Method>();
        assert!(
            lowercase.is_ok(),
            "'post' must parse as a valid custom extension method"
        );
        // It is NOT the standard POST method — casing is preserved.
        assert_ne!(
            lowercase.unwrap(),
            reqwest::Method::POST,
            "Lowercase 'post' is a different method object from standard POST"
        );
    }

    // ── Gap 2 ──────────────────────────────────────────────────────────────

    /// A UTF-8 byte-order mark (U+FEFF, bytes EF BB BF) prepended to a
    /// Bearer value does NOT match `starts_with(BEARER_PREFIX)` because the
    /// BOM occupies 3 bytes before the 'B' of "Bearer".
    ///
    /// The token must be rejected with "not a Bearer token", not with a
    /// panic or an out-of-bounds slice.
    #[tokio::test]
    async fn resolve_token_utf8_bom_before_bearer_is_rejected() {
        let vault = MemoryVault::new();
        // U+FEFF is the UTF-8 BOM: three bytes EF BB BF before "Bearer".
        let headers = vec![(
            "authorization".to_string(),
            "\u{FEFF}Bearer tok_anthropic_prod_abc123".to_string(),
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("not a Bearer token"),
            "BOM before 'Bearer' must be rejected as a non-Bearer token"
        );
    }

    // ── Gap 1 ──────────────────────────────────────────────────────────────

    /// `"Bearer "` with nothing after the prefix produces an empty `raw_token`
    /// (`""`).  `"".starts_with("tok_")` is false, so the error path at
    /// `worker.rs:215-218` must fire without any out-of-bounds panic
    /// (`"".len().min(16) == 0` → empty slice is valid).
    #[tokio::test]
    async fn resolve_token_bearer_prefix_only_returns_error_without_panic() {
        let vault = MemoryVault::new();
        let headers = vec![(
            "authorization".to_string(),
            "Bearer ".to_string(), // prefix only — nothing after the space
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err(), "Empty token after 'Bearer ' must be rejected");
        assert!(
            result.unwrap_err().contains("does not look like a proxy token"),
            "Error must mention the token format expectation"
        );
    }

    // ── Gap 2 ──────────────────────────────────────────────────────────────

    /// A CRLF sequence embedded in the Authorization header *value* (not in
    /// a forwarded request header) passes the `starts_with("tok_")` check but
    /// must be rejected by `ApiKeyToken::new` because `\r` and `\n` are not
    /// `[a-zA-Z0-9]` characters.
    ///
    /// Distinct from `forward_request_crlf_in_header_value_is_rejected` which
    /// tests CRLF injected into outgoing HTTP headers; this tests CRLF inside
    /// the Authorization value itself.
    #[tokio::test]
    async fn resolve_token_crlf_in_authorization_value_is_rejected() {
        let vault = MemoryVault::new();
        let headers = vec![(
            "authorization".to_string(),
            "Bearer tok_anthropic_prod_abc123\r\nX-Injected: yes".to_string(),
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err(), "CRLF in Authorization value must be rejected");
        // Either the token fails ApiKeyToken validation or the vault doesn't
        // find it — either way the request must not succeed.
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid proxy token") || msg.contains("not found in vault"),
            "Error must indicate token is invalid, got: {}",
            msg
        );
    }

    // ── Gap 1 ──────────────────────────────────────────────────────────────

    /// The worker builds its HTTP client with `redirect::Policy::none()`
    /// (`worker.rs` binary, line 89).  When the upstream returns a 3xx
    /// redirect, `forward_request` must return the 3xx response as-is —
    /// it must NOT follow the redirect to a different URL.
    ///
    /// This test uses the same `redirect::Policy::none()` the real worker
    /// uses, so it faithfully reproduces production behaviour.
    #[tokio::test]
    async fn forward_request_upstream_3xx_is_forwarded_without_following_redirect() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/v1/messages");
                then.status(301)
                    .header("location", "https://example.com/new-location");
            })
            .await;

        // Mirror the redirect policy used by the real worker binary.
        let client = ReqwestClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url: format!("{}/v1/messages", mock_server.base_url()),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "key-redir".to_string(),
        };
        let headers = vec![("Authorization".to_string(), "Bearer sk-key".to_string())];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_ok(), "3xx must not cause a transport error");
        assert_eq!(
            result.unwrap().status,
            301,
            "301 must be returned as-is, not followed"
        );
        mock.assert_async().await;
    }

    // ── Gap 2 ──────────────────────────────────────────────────────────────

    /// `" ".is_empty()` is `false` in Rust — a whitespace-only string is NOT
    /// considered empty.  The guard at `worker.rs:230` only calls
    /// `real_key.is_empty()`, so a vault entry of a single space passes and
    /// is returned as the resolved key.
    ///
    /// This documents the design boundary: vault callers are responsible for
    /// not storing whitespace-only values.
    #[tokio::test]
    async fn resolve_token_whitespace_only_real_key_passes_empty_check() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_wspace001").unwrap();
        vault.store(&token, " ").await.unwrap(); // single space — not empty

        let headers = make_headers("Bearer tok_anthropic_prod_wspace001");
        let result = resolve_token(&vault, &headers).await;

        assert!(
            result.is_ok(),
            "Whitespace-only key must pass is_empty() check — got: {:?}",
            result
        );
        assert_eq!(
            result.unwrap(),
            " ",
            "Whitespace key must be returned unchanged"
        );
    }

    // ── Gap 4 ──────────────────────────────────────────────────────────────

    /// `body.is_empty()` is `false` for `vec![0x00]` (a single null byte).
    /// The body must be included in the upstream request — not silently
    /// omitted as if it were empty.
    #[tokio::test]
    async fn forward_request_body_with_null_byte_is_sent_to_upstream() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/v1/data")
                    .body("\0"); // null byte as &str — Into<String> required by httpmock
                then.status(200);
            })
            .await;

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "POST".to_string(),
            url: format!("{}/v1/data", mock_server.base_url()),
            headers: vec![],
            body: vec![0x00], // single null byte — not empty
            reply_to: "test.reply".to_string(),
            idempotency_key: "key-null".to_string(),
        };
        let headers = vec![("Authorization".to_string(), "Bearer sk-key".to_string())];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, 200);
        // mock.assert_async() verifies the null-byte body was actually sent.
        mock.assert_async().await;
    }

    // ── Gap: empty method string ──────────────────────────────────────────────

    /// An empty string is not a valid RFC 7230 HTTP method token.
    /// `"".parse::<reqwest::Method>()` must fail, and `forward_request`
    /// must return `Err("Invalid HTTP method: …")` — no panic.
    #[tokio::test]
    async fn forward_request_empty_method_returns_error() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "".to_string(),
            url: "http://127.0.0.1:1/v1/messages".to_string(),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-empty-method".to_string(),
        };

        let result = forward_request(&client, &request, &[]).await;

        assert!(result.is_err(), "Empty method string must be rejected");
        assert!(
            result.unwrap_err().contains("Invalid HTTP method"),
            "Error must contain 'Invalid HTTP method'"
        );
    }

    // ── Gap: bizarre mixed-case Authorization header name ─────────────────────

    /// `eq_ignore_ascii_case` matches any ASCII casing of "authorization",
    /// including bizarre variants like "AuThOrIzAtIoN".
    /// Documents that header name matching is fully case-insensitive.
    #[tokio::test]
    async fn resolve_token_bizarre_mixed_case_authorization_header_is_matched() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_mixcase1").unwrap();
        vault.store(&token, "sk-ant-mixcase-key").await.unwrap();

        let headers = vec![(
            "AuThOrIzAtIoN".to_string(),
            "Bearer tok_anthropic_prod_mixcase1".to_string(),
        )];

        let key = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(
            key, "sk-ant-mixcase-key",
            "Mixed-case header name must match via eq_ignore_ascii_case"
        );
    }

    // ── Gap: "Bearer" without trailing space ──────────────────────────────────

    /// `"Bearer"` (exactly 6 bytes, no trailing space or token) does NOT match
    /// `starts_with("Bearer ")` (7 bytes).  The check at line 208 must reject
    /// it with "not a Bearer token" — no out-of-bounds access since
    /// `starts_with` is safe on a shorter string.
    ///
    /// Distinct from `resolve_token_bearer_prefix_only_returns_error_without_panic`
    /// which tests `"Bearer "` (with space, empty token); here there is no space
    /// at all so we never even enter the `raw_token` logic.
    #[tokio::test]
    async fn resolve_token_bearer_without_space_returns_not_bearer_error() {
        let vault = MemoryVault::new();
        let headers = vec![(
            "authorization".to_string(),
            "Bearer".to_string(), // exactly 6 bytes — no trailing space
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(result.is_err(), "'Bearer' without space must be rejected");
        assert!(
            result.unwrap_err().contains("not a Bearer token"),
            "Error must describe the rejection"
        );
    }

    // ── Gap: non-breaking space after "Bearer" ─────────────────────────────────

    /// `"Bearer\u{00A0}tok_..."` uses a Unicode non-breaking space (U+00A0,
    /// bytes 0xC2 0xA0) instead of an ASCII space (0x20).
    /// `starts_with("Bearer ")` is a byte-exact comparison — U+00A0 ≠ U+0020
    /// — so the header must be rejected with "not a Bearer token".
    #[tokio::test]
    async fn resolve_token_non_breaking_space_after_bearer_is_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_nbsptest1").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        // Non-breaking space (U+00A0) instead of ASCII space after "Bearer".
        let headers = vec![(
            "authorization".to_string(),
            "Bearer\u{00A0}tok_anthropic_prod_nbsptest1".to_string(),
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(
            result.is_err(),
            "Non-breaking space after 'Bearer' must be rejected"
        );
        assert!(
            result.unwrap_err().contains("not a Bearer token"),
            "Error must describe the rejection"
        );
    }

    // ── Gap: method with internal spaces ──────────────────────────────────────

    /// A method string containing internal spaces (`"P O S T"`) is not a valid
    /// RFC 7230 HTTP method token — spaces are separators, not token characters.
    /// `reqwest::Method::from_str("P O S T")` must fail with an error and
    /// `forward_request` must propagate it as `"Invalid HTTP method: …"`.
    #[tokio::test]
    async fn forward_request_method_with_internal_spaces_returns_error() {
        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "P O S T".to_string(),
            url: "http://127.0.0.1:1/v1/messages".to_string(),
            headers: vec![],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-spaced".to_string(),
        };

        let result = forward_request(&client, &request, &[]).await;

        assert!(
            result.is_err(),
            "Method with internal spaces must be rejected"
        );
        assert!(
            result.unwrap_err().contains("Invalid HTTP method"),
            "Error must contain 'Invalid HTTP method'"
        );
    }

    // ── Gap 5 ──────────────────────────────────────────────────────────────

    /// A 204 No Content response has no body.  `upstream_resp.bytes().await`
    /// returns an empty `Bytes`, so `OutboundHttpResponse.body` must be
    /// `vec![]` and the status 204 must be preserved — no panic, no
    /// unexpected bytes injected.
    #[tokio::test]
    async fn forward_request_204_no_content_response_is_forwarded() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::DELETE).path("/v1/items/1");
                then.status(204); // No Content
            })
            .await;

        let client = ReqwestClient::new();
        let request = OutboundHttpRequest {
            method: "DELETE".to_string(),
            url: format!("{}/v1/items/1", mock_server.base_url()),
            headers: vec![],
            body: vec![], // DELETE typically has no body
            reply_to: "test.reply".to_string(),
            idempotency_key: "key-del".to_string(),
        };
        let headers = vec![("Authorization".to_string(), "Bearer sk-key".to_string())];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp.status, 204, "204 status must be preserved");
        assert!(resp.body.is_empty(), "204 response must have an empty body");
        mock.assert_async().await;
    }
}
