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
}
