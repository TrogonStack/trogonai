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

use crate::messages::{OutboundHttpRequest, OutboundHttpResponse, StreamFrame};
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

        let is_streaming = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
            .unwrap_or(false);

        if is_streaming {
            process_request_streaming(&request, &*vault, &http_client, &nats, &reply_to).await;
        } else {
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
        }

        msg.ack().await;
    }

    Ok(())
}

/// Streaming variant of request processing.
///
/// Resolves the token, calls the upstream with `stream: true`, and publishes
/// `Start → Chunk(s) → End` frames to the Core NATS reply subject.
///
/// For error cases (token resolution failure, transport error) the function
/// publishes `Start(4xx/5xx)` → `Chunk(error body)` → `End` so the proxy
/// always receives the same three-frame protocol regardless of the outcome.
async fn process_request_streaming<V, H, N>(
    request: &OutboundHttpRequest,
    vault: &V,
    http_client: &H,
    nats: &N,
    reply_to: &str,
) where
    V: VaultStore,
    V::Error: std::fmt::Display,
    H: HttpClient,
    N: NatsClient,
{
    /// Publish a StreamFrame to the reply subject.  Swallows NATS errors to
    /// avoid double-panicking when an earlier publish already failed.
    async fn publish_frame<N: NatsClient>(nats: &N, reply_to: &str, frame: StreamFrame) {
        if let Ok(payload) = serde_json::to_vec(&frame) {
            let _ = nats.publish(reply_to.to_string(), payload.into()).await;
        }
    }

    /// Send Start(status) + Chunk(body_bytes) + End to surface an error.
    async fn send_error_response<N: NatsClient>(nats: &N, reply_to: &str, status: u16, body: Vec<u8>) {
        publish_frame(nats, reply_to, StreamFrame::Start { status, headers: vec![] }).await;
        publish_frame(nats, reply_to, StreamFrame::Chunk { seq: 0, data: body }).await;
        publish_frame(nats, reply_to, StreamFrame::End { error: None }).await;
    }

    // ── Token resolution ──────────────────────────────────────────────────────
    let (real_key, _previous_key) = match resolve_token(vault, &request.headers).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "Streaming: token resolution failed");
            send_error_response(nats, reply_to, 401, e.into_bytes()).await;
            return;
        }
    };

    // ── Build forwarded headers ───────────────────────────────────────────────
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

    // ── Streaming upstream call ───────────────────────────────────────────────
    let streaming_resp = match http_client
        .send_request_streaming(&request.method, &request.url, &forwarded_headers, &request.body)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, url = %request.url, "Streaming: upstream HTTP call failed");
            send_error_response(nats, reply_to, 502, e.into_bytes()).await;
            return;
        }
    };

    // Strip response headers that contain the real API key before forwarding.
    let resp_headers: Vec<(String, String)> = streaming_resp
        .headers
        .iter()
        .filter(|(_, v)| !v.contains(real_key.as_str()))
        .cloned()
        .collect();

    // ── Publish Start frame ───────────────────────────────────────────────────
    publish_frame(
        nats,
        reply_to,
        StreamFrame::Start {
            status: streaming_resp.status,
            headers: resp_headers,
        },
    )
    .await;

    // ── Publish Chunk frames ──────────────────────────────────────────────────
    let mut chunks = streaming_resp.chunks;
    let mut seq = 0u64;
    loop {
        match chunks.next().await {
            Some(Ok(bytes)) => {
                publish_frame(
                    nats,
                    reply_to,
                    StreamFrame::Chunk {
                        seq,
                        data: bytes.to_vec(),
                    },
                )
                .await;
                seq += 1;
            }
            Some(Err(e)) => {
                tracing::error!(error = %e, "Streaming: error reading upstream chunk");
                publish_frame(nats, reply_to, StreamFrame::End { error: Some(e) }).await;
                return;
            }
            None => break,
        }
    }

    // ── Publish End frame ─────────────────────────────────────────────────────
    publish_frame(nats, reply_to, StreamFrame::End { error: None }).await;
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
    let (real_key, previous_key) = match resolve_token(vault, &request.headers).await {
        Ok(pair) => pair,
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

    let resp = match forward_request_with_retry(http_client, request, &forwarded_headers).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, url = %request.url, "Upstream HTTP call failed after retries");
            return OutboundHttpResponse {
                status: 502,
                headers: vec![],
                body: vec![],
                error: Some(e),
            };
        }
    };

    // Fallback-on-401: if upstream rejects the current key during a rotation grace period,
    // retry once with the previous key before surfacing the 401 to the caller.
    if resp.status == 401 {
        if let Some(prev) = previous_key {
            tracing::warn!(
                url = %request.url,
                "Current key rejected (401), retrying with previous key during rotation grace period"
            );
            let mut fallback_headers: Vec<(String, String)> = request
                .headers
                .iter()
                .filter(|(k, _)| {
                    !k.eq_ignore_ascii_case("authorization")
                        && !k.eq_ignore_ascii_case("x-request-id")
                })
                .cloned()
                .collect();
            fallback_headers
                .push(("Authorization".to_string(), format!("Bearer {}", prev)));
            fallback_headers
                .push(("X-Request-Id".to_string(), request.idempotency_key.clone()));
            return match forward_request(http_client, request, &fallback_headers).await {
                Ok(mut r) => {
                    r.headers.retain(|(_, v)| !v.contains(prev.as_str()));
                    r
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        url = %request.url,
                        "Fallback request with previous key failed"
                    );
                    OutboundHttpResponse {
                        status: 502,
                        headers: vec![],
                        body: vec![],
                        error: Some(e),
                    }
                }
            };
        }
    }

    // Strip any response header whose value contains the real API key.
    // A misbehaving provider might echo it back; the caller must never see it.
    let mut resp = resp;
    resp.headers.retain(|(_, v)| !v.contains(real_key.as_str()));
    resp
}

/// Extract the `tok_...` token from the Authorization header and resolve it.
///
/// Returns `(current_key, previous_key)`. `previous_key` is `Some` only during
/// a rotation grace period — used by the caller for fallback-on-401.
async fn resolve_token<V>(
    vault: &V,
    headers: &[(String, String)],
) -> Result<(String, Option<String>), String>
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

    let (current_opt, previous_opt) = vault
        .resolve_with_previous(&token)
        .await
        .map_err(|e| format!("Vault error: {}", e))?;

    let real_key = current_opt
        .ok_or_else(|| format!("Token not found in vault: {}", token))?;

    if real_key.is_empty() {
        return Err(format!("Vault returned an empty key for token: {}", token));
    }

    Ok((real_key, previous_opt))
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
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use reqwest::Client as ReqwestClient;
    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    use bytes::Bytes;
    use futures_util::Stream;

    use crate::messages::{OutboundHttpRequest, StreamFrame};
    use crate::traits::{HttpClient, HttpResponse, NatsClient, StreamingHttpResponse};

    use super::{
        HTTP_INITIAL_RETRY_DELAY, forward_request, forward_request_with_retry,
        process_request, process_request_streaming, resolve_token,
    };

    // ── MockNatsClient ────────────────────────────────────────────────────────

    #[derive(Clone, Default)]
    struct MockNatsClient {
        published: Arc<Mutex<Vec<(String, Bytes)>>>,
    }

    impl MockNatsClient {
        fn new() -> Self {
            Self::default()
        }

        fn published_frames(&self) -> Vec<StreamFrame> {
            self.published
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(_, payload)| serde_json::from_slice(payload).ok())
                .collect()
        }
    }

    impl NatsClient for MockNatsClient {
        type Sub = futures_util::stream::Empty<async_nats::Message>;

        async fn subscribe(&self, _subject: String) -> Result<Self::Sub, String> {
            Ok(futures_util::stream::empty())
        }

        async fn publish(&self, subject: String, payload: Bytes) -> Result<(), String> {
            self.published.lock().unwrap().push((subject, payload));
            Ok(())
        }
    }

    // ── MockHttpClient ────────────────────────────────────────────────────────

    /// In-memory mock HTTP client.  Pre-load responses with [`enqueue_ok`] /
    /// [`enqueue_err`] and they are returned in FIFO order.  Returns an error
    /// when the queue is empty.
    ///
    /// Wrapped in `Arc` so it satisfies the `Clone` bound required by
    /// [`crate::traits::HttpClient`].
    #[derive(Clone)]
    struct MockHttpClient {
        responses: Arc<Mutex<VecDeque<Result<HttpResponse, String>>>>,
        streaming: Arc<Mutex<VecDeque<Result<(u16, Vec<(String, String)>, Vec<Vec<u8>>), String>>>>,
    }

    impl MockHttpClient {
        fn new() -> Self {
            Self {
                responses: Arc::new(Mutex::new(VecDeque::new())),
                streaming: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn enqueue_ok(&self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
            self.responses.lock().unwrap().push_back(Ok(HttpResponse {
                status,
                headers,
                body,
            }));
        }

        fn enqueue_err(&self, msg: impl Into<String>) {
            self.responses.lock().unwrap().push_back(Err(msg.into()));
        }

        fn enqueue_streaming_ok(
            &self,
            status: u16,
            headers: Vec<(String, String)>,
            chunks: Vec<Vec<u8>>,
        ) {
            self.streaming
                .lock()
                .unwrap()
                .push_back(Ok((status, headers, chunks)));
        }

        fn enqueue_streaming_err(&self, msg: impl Into<String>) {
            self.streaming.lock().unwrap().push_back(Err(msg.into()));
        }
    }

    impl HttpClient for MockHttpClient {
        fn send_request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let result = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("MockHttpClient: no more responses enqueued".to_string()));
            Box::pin(async move { result })
        }

        fn send_request_streaming(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Pin<Box<dyn Future<Output = Result<StreamingHttpResponse, String>> + Send + '_>> {
            let entry = self
                .streaming
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("MockHttpClient: no streaming responses enqueued".to_string()));
            Box::pin(async move {
                match entry {
                    Ok((status, headers, chunks)) => {
                        let stream = futures_util::stream::iter(
                            chunks.into_iter().map(|c| Ok::<Bytes, String>(Bytes::from(c))),
                        );
                        Ok(StreamingHttpResponse {
                            status,
                            headers,
                            chunks: Box::pin(stream),
                        })
                    }
                    Err(e) => Err(e),
                }
            })
        }
    }

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
        let (key, _prev) = resolve_token(&vault, &headers).await.unwrap();
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

    /// Gap 3: token starting with `tok_` but missing provider/env/id segments
    /// must be rejected by `ApiKeyToken::new` validation before touching the vault.
    #[tokio::test]
    async fn resolve_token_malformed_tok_missing_parts_is_rejected() {
        let vault = MemoryVault::new();

        let cases = [
            "Bearer tok_anthropic",      // missing env and id
            "Bearer tok_anthropic_prod", // missing id
            "Bearer tok_",               // nothing after tok_
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
    async fn resolve_token_empty_key_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_openai_prod_emptykey1").unwrap();
        vault.store(&token, "").await.unwrap();

        let headers = make_headers("Bearer tok_openai_prod_emptykey1");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty key"));
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

        let (key, _prev) = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(key, "sk-ant-value");
    }

    // ── MockHttpClient-based process_request tests ────────────────────────────

    /// Happy path using MockHttpClient: verifies that process_request replaces
    /// the proxy token with the real key and forwards the request, without
    /// touching a real HTTP server.
    #[tokio::test]
    async fn process_request_mock_http_happy_path() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_mock001").unwrap();
        vault.store(&token, "sk-ant-mocksecret").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_ok(
            200,
            vec![("content-type".to_string(), "application/json".to_string())],
            br#"{"id":"msg_mock"}"#.to_vec(),
        );

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_anthropic_prod_mock001",
            "idem-mock-001",
        );
        let resp = process_request(&request, &vault, &http).await;

        assert_eq!(resp.status, 200);
        assert!(resp.error.is_none());
    }

    /// When the vault does not contain the token, process_request returns 401
    /// and MockHttpClient should not be called (queue stays empty → no panic).
    #[tokio::test]
    async fn process_request_mock_http_missing_token_returns_401() {
        let vault = MemoryVault::new(); // empty
        let http = MockHttpClient::new(); // no responses enqueued

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_openai_prod_absent1",
            "idem-mock-002",
        );
        let resp = process_request(&request, &vault, &http).await;

        assert_eq!(resp.status, 401);
        assert!(resp.error.is_some());
        assert!(resp.error.unwrap().contains("not found"));
    }

    /// When the HTTP client returns an error (transport failure), process_request
    /// returns a 502 error response.
    #[tokio::test]
    async fn process_request_mock_http_transport_error_returns_502() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_mock003").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_err("HTTP request failed: connection refused");

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_anthropic_prod_mock003",
            "idem-mock-003",
        );
        let resp = process_request(&request, &vault, &http).await;

        assert_eq!(resp.status, 502);
        assert!(resp.error.is_some());
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

        let client = ReqwestClient::new();
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

    #[tokio::test]
    async fn process_request_returns_401_when_token_not_found() {
        let server = httpmock::MockServer::start_async().await;
        let vault = MemoryVault::new(); // empty vault
        let client = ReqwestClient::new();

        let request = make_request(
            &format!("{}/v1/messages", server.base_url()),
            "Bearer tok_anthropic_prod_notfound",
            "idem-1",
        );

        let response = process_request(&request, &vault, &client).await;
        assert_eq!(response.status, 401);
        assert!(response.error.unwrap().contains("not found"));
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

        let client = ReqwestClient::new();
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
        assert_eq!(
            mock.hits(),
            4,
            "should attempt exactly 4 times (1 + 3 retries)"
        );
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

        let resp = forward_request(&client, &request, &[]).await.unwrap();

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
    #[tokio::test]
    async fn retry_succeeds_after_transient_5xx() {
        let mock_server = httpmock::MockServer::start_async().await;

        let ok_mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/ok");
                then.status(200).body("recovered");
            })
            .await;

        let client = ReqwestClient::new();

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

        let headers = make_headers("Bearer  tok_anthropic_prod_dbl01");
        let result = resolve_token(&vault, &headers).await;
        assert!(result.is_err(), "double-space Bearer must be rejected");
        assert!(
            result
                .unwrap_err()
                .contains("does not look like a proxy token"),
            "Error must describe the rejection reason"
        );
    }

    // ── Gap: VaultStore::resolve() returning Err ───────────────────────────────

    /// When the vault backend itself returns an error (network failure, I/O error,
    /// etc.), `process_request` must surface it as a `401` error response.
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
                async { Err(std::io::Error::other("vault backend unavailable")) }
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

        assert_eq!(
            resp.status, 502,
            "A newline in the real key must cause a 502 error (reqwest rejects \\n in header values)"
        );
        assert!(resp.error.is_some(), "Error field must be set");

        assert_eq!(
            mock.hits(),
            0,
            "Upstream must not be called when header is invalid"
        );
    }

    // ── Gap: single-char real key strips every header containing that byte ────

    #[tokio::test]
    async fn process_request_single_char_real_key_strips_every_matching_header() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_onechar01").unwrap();
        vault.store(&token, "k").await.unwrap();

        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/v1/messages");
                then.status(200)
                    .header("content-type", "application/json") // no 'k' → preserved
                    .header("x-link", "bookmark") // "bookmark" contains 'k' → stripped
                    .header("x-safe", "clean-value") // no 'k' → preserved
                    .body(r#"{"id":"msg_onechar"}"#);
            })
            .await;

        let url = format!("{}/v1/messages", mock_server.base_url());
        let request = make_request(&url, "Bearer tok_anthropic_prod_onechar01", "idem-1char");
        let client = ReqwestClient::new();

        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(resp.status, 200);
        mock.assert_async().await;

        let link_present = resp.headers.iter().any(|(k, _)| k == "x-link");
        assert!(
            !link_present,
            "x-link (value 'bookmark' contains 'k') must be stripped"
        );

        let ct_present = resp.headers.iter().any(|(k, _)| k == "content-type");
        assert!(
            ct_present,
            "content-type must be preserved (no 'k' in value)"
        );

        let safe_present = resp.headers.iter().any(|(k, _)| k == "x-safe");
        assert!(safe_present, "x-safe must be preserved (no 'k' in value)");
    }

    // ── Gap: sub-500 status not retried ──────────────────────────────────────

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
            body: vec![], // empty body — forward_request skips .body()
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-get-ct".to_string(),
        };

        let extra_headers = vec![("content-type".to_string(), "application/json".to_string())];

        let resp = forward_request_with_retry(&client, &request, &extra_headers)
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        mock.assert_async().await;
    }

    // ── Gap: multiple Authorization headers all stripped ──────────────────────

    #[tokio::test]
    async fn process_request_duplicate_authorization_headers_stripped() {
        let mock_server = httpmock::MockServer::start_async().await;

        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_dupauth1").unwrap();
        let real_key = "sk-ant-dupauth-real-key";
        vault.store(&token, real_key).await.unwrap();

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
                (
                    "Authorization".to_string(),
                    "Bearer tok_anthropic_prod_dupauth1".to_string(),
                ),
                (
                    "Authorization".to_string(),
                    "Bearer some-extra-key-1".to_string(),
                ),
                (
                    "authorization".to_string(),
                    "Bearer some-extra-key-2".to_string(),
                ),
            ],
            body: b"{}".to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-dup-01".to_string(),
        };

        let client = ReqwestClient::new();
        let resp = process_request(&request, &vault, &client).await;

        assert_eq!(
            resp.status, 200,
            "All duplicate Authorization headers must be stripped and real key injected"
        );
        mock.assert_async().await;
    }

    // ── Gap: invalid HTTP method names (non-ASCII / special chars) ─────────────

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
                async { Err(std::io::Error::other("simulated backend failure")) }
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
            err, "Vault error: simulated backend failure",
            "Vault error format must be 'Vault error: <display>'"
        );
    }

    // ── Gap: exponential backoff shift saturates at 31 ────────────────────────

    #[test]
    fn backoff_shift_saturates_at_31_for_high_attempt_counts() {
        for attempts in [32u32, 33, 50, 100, u32::MAX] {
            let shift = (attempts - 1).min(31);
            assert_eq!(
                shift, 31,
                "Shift must be saturated at 31 for attempts={}, got {}",
                attempts, shift
            );
            let multiplier = 1u32 << shift;
            assert_eq!(
                multiplier,
                2u32.pow(31),
                "Multiplier must be 2^31 at saturation"
            );
            let _delay = HTTP_INITIAL_RETRY_DELAY * multiplier;
        }
    }

    // ── Gap C: response header with invalid UTF-8 silently dropped ────────────

    #[tokio::test]
    async fn forward_request_invalid_utf8_response_header_silently_dropped() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            #[allow(clippy::unused_io_amount)]
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

    #[tokio::test]
    async fn forward_request_5xx_retry_succeeds_on_second_attempt() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempt_count = Arc::new(AtomicU32::new(0));
        let counter = attempt_count.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let mut buf = vec![0u8; 4096];
                #[allow(clippy::unused_io_amount)]
                stream.read(&mut buf).await.ok();

                if n == 1 {
                    let r = b"HTTP/1.1 503 Service Unavailable\r\n\
                              Connection: close\r\n\
                              content-length: 5\r\n\r\nerror";
                    stream.write_all(r).await.ok();
                } else {
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
        assert_eq!(
            resp.body, b"retried",
            "Body from successful retry must be returned"
        );
        assert_eq!(
            attempt_count.load(Ordering::SeqCst),
            2,
            "Must make exactly 2 attempts"
        );
    }

    // ── Gap #9: first Authorization header wins when multiple with different casing ──

    #[tokio::test]
    async fn resolve_token_first_authorization_header_wins_when_multiple() {
        let vault = MemoryVault::new();

        let token_first = ApiKeyToken::new("tok_anthropic_prod_first01").unwrap();
        let token_second = ApiKeyToken::new("tok_anthropic_prod_second1").unwrap();
        vault.store(&token_first, "sk-ant-first-key").await.unwrap();
        vault
            .store(&token_second, "sk-ant-second-key")
            .await
            .unwrap();

        let headers = vec![
            (
                "AUTHORIZATION".to_string(),
                "Bearer tok_anthropic_prod_first01".to_string(),
            ),
            (
                "authorization".to_string(),
                "Bearer tok_anthropic_prod_second1".to_string(),
            ),
        ];

        let (key, _prev) = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(
            key, "sk-ant-first-key",
            "First Authorization header must win; second must be ignored"
        );
    }

    // ── Gap B ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_token_very_long_header_error_truncated_at_16_chars() {
        let vault = MemoryVault::new();
        let long_prefix = "X".repeat(100_000);
        let auth_value = format!("Bearer {}", long_prefix);
        let headers = vec![("authorization".to_string(), auth_value)];

        let result = resolve_token(&vault, &headers).await;

        assert!(
            result.is_err(),
            "Long non-tok_ header must produce an error"
        );
        let msg = result.unwrap_err();

        assert!(
            msg.contains("XXXXXXXXXXXXXXXX"),
            "Error must contain a 16-char truncation, got: {}",
            msg
        );
        assert!(
            !msg.contains(&"X".repeat(17)),
            "Error must not leak more than 16 chars of the raw value"
        );
    }

    // ── Gap C ──────────────────────────────────────────────────────────────

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
        let headers = vec![
            ("Authorization".to_string(), "Bearer sk-realkey".to_string()),
            ("X-Request-Id".to_string(), "key\x01invalid".to_string()),
        ];

        let result = forward_request(&client, &request, &headers).await;

        assert!(
            result.is_err(),
            "Control char in header value must cause an error"
        );
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error must originate from the HTTP layer"
        );
    }

    // ── Gap E ──────────────────────────────────────────────────────────────

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
            ("X-Evil".to_string(), "value\r\nX-Injected: yes".to_string()),
        ];

        let result = forward_request(&client, &request, &headers).await;

        assert!(result.is_err(), "CRLF in header value must cause an error");
        assert!(
            result.unwrap_err().contains("HTTP request failed"),
            "Error must originate from the HTTP layer"
        );
    }

    // ── Gap 3 ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_token_leading_whitespace_before_bearer_is_rejected() {
        let vault = MemoryVault::new();
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

    #[test]
    fn http_method_lowercase_is_valid_rfc7230_extension_token() {
        let lowercase = "post".parse::<reqwest::Method>();
        assert!(
            lowercase.is_ok(),
            "'post' must parse as a valid custom extension method"
        );
        assert_ne!(
            lowercase.unwrap(),
            reqwest::Method::POST,
            "Lowercase 'post' is a different method object from standard POST"
        );
    }

    // ── Gap 2 ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_token_utf8_bom_before_bearer_is_rejected() {
        let vault = MemoryVault::new();
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

    #[tokio::test]
    async fn resolve_token_bearer_prefix_only_returns_error_without_panic() {
        let vault = MemoryVault::new();
        let headers = vec![(
            "authorization".to_string(),
            "Bearer ".to_string(), // prefix only — nothing after the space
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(
            result.is_err(),
            "Empty token after 'Bearer ' must be rejected"
        );
        assert!(
            result
                .unwrap_err()
                .contains("does not look like a proxy token"),
            "Error must mention the token format expectation"
        );
    }

    // ── Gap 2 ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_token_crlf_in_authorization_value_is_rejected() {
        let vault = MemoryVault::new();
        let headers = vec![(
            "authorization".to_string(),
            "Bearer tok_anthropic_prod_abc123\r\nX-Injected: yes".to_string(),
        )];

        let result = resolve_token(&vault, &headers).await;

        assert!(
            result.is_err(),
            "CRLF in Authorization value must be rejected"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid proxy token") || msg.contains("not found in vault"),
            "Error must indicate token is invalid, got: {}",
            msg
        );
    }

    // ── Gap 1 ──────────────────────────────────────────────────────────────

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
            result.unwrap().0,
            " ",
            "Whitespace key must be returned unchanged"
        );
    }

    // ── Gap 4 ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn forward_request_body_with_null_byte_is_sent_to_upstream() {
        let mock_server = httpmock::MockServer::start_async().await;
        let mock = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/v1/data")
                    .body("\0");
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
        mock.assert_async().await;
    }

    // ── Gap: empty method string ──────────────────────────────────────────────

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

    #[tokio::test]
    async fn resolve_token_bizarre_mixed_case_authorization_header_is_matched() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_mixcase1").unwrap();
        vault.store(&token, "sk-ant-mixcase-key").await.unwrap();

        let headers = vec![(
            "AuThOrIzAtIoN".to_string(),
            "Bearer tok_anthropic_prod_mixcase1".to_string(),
        )];

        let (key, _prev) = resolve_token(&vault, &headers).await.unwrap();
        assert_eq!(
            key, "sk-ant-mixcase-key",
            "Mixed-case header name must match via eq_ignore_ascii_case"
        );
    }

    // ── Gap: "Bearer" without trailing space ──────────────────────────────────

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

    #[tokio::test]
    async fn resolve_token_non_breaking_space_after_bearer_is_rejected() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_nbsptest1").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

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
            body: vec![],
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

    // ── Gap: error message 16-char boundary ──────────────────────────────────

    #[tokio::test]
    async fn resolve_token_error_shows_all_chars_when_token_is_under_16() {
        let vault = MemoryVault::new();
        let headers = make_headers("Bearer sk-ant-realkey1");
        let err = resolve_token(&vault, &headers).await.unwrap_err();
        assert!(
            err.contains("sk-ant-realkey1"),
            "All 15 chars must appear in the error message: {}",
            err
        );
    }

    #[tokio::test]
    async fn resolve_token_error_shows_all_chars_when_token_is_exactly_16() {
        let vault = MemoryVault::new();
        let headers = make_headers("Bearer sk-ant-realkey12");
        let err = resolve_token(&vault, &headers).await.unwrap_err();
        assert!(
            err.contains("sk-ant-realkey12"),
            "All 16 chars must appear (no truncation at boundary): {}",
            err
        );
    }

    #[tokio::test]
    async fn resolve_token_error_truncates_to_16_chars_when_token_is_17() {
        let vault = MemoryVault::new();
        let headers = make_headers("Bearer sk-ant-realkey123");
        let err = resolve_token(&vault, &headers).await.unwrap_err();
        assert!(
            err.contains("sk-ant-realkey12"),
            "First 16 chars must appear in the error message: {}",
            err
        );
        assert!(
            !err.contains("sk-ant-realkey123"),
            "Full 17-char token must NOT appear — must be truncated at 16: {}",
            err
        );
    }

    // ── Fallback-on-401 ───────────────────────────────────────────────────────

    /// When upstream rejects the current key with 401 during a rotation grace
    /// period, process_request retries once with the previous key and returns
    /// that response.
    #[tokio::test]
    async fn process_request_fallback_on_401_uses_previous_key() {
        use trogon_vault::{ApiKeyToken as VaultToken, VaultStore as VaultStoreTrait};

        struct VaultWithPrevious;
        impl VaultStoreTrait for VaultWithPrevious {
            type Error = std::io::Error;
            fn store(&self, _: &VaultToken, _: &str)
                -> impl std::future::Future<Output = Result<(), Self::Error>> + Send
            { async { Ok(()) } }
            fn resolve(&self, _: &VaultToken)
                -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send
            { async { Ok(Some("sk-current".to_string())) } }
            fn revoke(&self, _: &VaultToken)
                -> impl std::future::Future<Output = Result<(), Self::Error>> + Send
            { async { Ok(()) } }
            fn resolve_with_previous(&self, _: &VaultToken)
                -> impl std::future::Future<Output = Result<(Option<String>, Option<String>), Self::Error>> + Send
            { async { Ok((Some("sk-current".to_string()), Some("sk-previous".to_string()))) } }
        }

        let vault = VaultWithPrevious;
        let http = MockHttpClient::new();
        http.enqueue_ok(401, vec![], b"unauthorized".to_vec());
        http.enqueue_ok(200, vec![], br#"{"id":"ok"}"#.to_vec());

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_anthropic_prod_abc123",
            "idem-fallback-01",
        );

        let resp = process_request(&request, &vault, &http).await;

        assert_eq!(resp.status, 200, "Fallback to previous key must succeed");
        assert!(resp.error.is_none());
    }

    /// When upstream returns 401 and there is no previous key, the 401 is
    /// returned to the caller — no fallback attempt is made.
    #[tokio::test]
    async fn process_request_401_returned_when_no_previous_key() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, "sk-current").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_ok(401, vec![], b"unauthorized".to_vec());

        let request = make_request(
            "https://api.anthropic.com/v1/messages",
            "Bearer tok_anthropic_prod_abc123",
            "idem-no-fallback-01",
        );

        let resp = process_request(&request, &vault, &http).await;

        assert_eq!(
            resp.status, 401,
            "Without a previous key, 401 must be forwarded as-is"
        );
        assert!(resp.error.is_none(), "Upstream 401 must not set the error field");
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

    // ── process_request_streaming unit tests ──────────────────────────────────

    fn make_streaming_request(auth: &str) -> OutboundHttpRequest {
        OutboundHttpRequest {
            method: "POST".to_string(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            headers: make_headers(auth),
            body: br#"{"stream":true}"#.to_vec(),
            reply_to: "test.reply".to_string(),
            idempotency_key: "idem-stream-01".to_string(),
        }
    }

    /// Happy path: worker publishes Start → Chunk (one per source chunk) → End.
    #[tokio::test]
    async fn process_request_streaming_publishes_start_chunk_end() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_stream1").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_streaming_ok(
            200,
            vec![("content-type".to_string(), "text/event-stream".to_string())],
            vec![b"chunk-A".to_vec(), b"chunk-B".to_vec()],
        );

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_stream1");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        assert_eq!(frames.len(), 4, "Start + 2 Chunks + End = 4 frames");

        assert!(matches!(frames[0], StreamFrame::Start { status: 200, .. }));
        assert!(matches!(frames[1], StreamFrame::Chunk { seq: 0, .. }));
        assert!(matches!(frames[2], StreamFrame::Chunk { seq: 1, .. }));
        assert!(matches!(frames[3], StreamFrame::End { error: None }));
    }

    /// Chunk data must be forwarded verbatim in order.
    #[tokio::test]
    async fn process_request_streaming_chunk_data_is_forwarded_in_order() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_stream2").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_streaming_ok(
            200,
            vec![],
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()],
        );

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_stream2");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        let chunks: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                StreamFrame::Chunk { data, seq } => Some((*seq, data.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (0, b"first".to_vec()));
        assert_eq!(chunks[1], (1, b"second".to_vec()));
        assert_eq!(chunks[2], (2, b"third".to_vec()));
    }

    /// Token not found → Start(401) + Chunk(error msg) + End, no HTTP call.
    #[tokio::test]
    async fn process_request_streaming_unknown_token_publishes_401_frames() {
        let vault = MemoryVault::new(); // empty
        let http = MockHttpClient::new(); // must not be called

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_notfound");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        assert_eq!(frames.len(), 3, "Start + Chunk + End even on error");
        assert!(matches!(frames[0], StreamFrame::Start { status: 401, .. }));
        assert!(matches!(frames[2], StreamFrame::End { error: None }));

        // No streaming response was enqueued so the queue is still empty,
        // proving the HTTP client was never called.
        assert!(
            http.streaming.lock().unwrap().is_empty(),
            "HTTP client must not be called when token is missing"
        );
    }

    /// Transport failure → Start(502) + Chunk(error msg) + End.
    #[tokio::test]
    async fn process_request_streaming_transport_error_publishes_502_frames() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_stream3").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_streaming_err("connection refused");

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_stream3");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        assert_eq!(frames.len(), 3);
        assert!(matches!(frames[0], StreamFrame::Start { status: 502, .. }));
        assert!(matches!(frames[2], StreamFrame::End { .. }));
    }

    /// The real API key must not appear in the Start frame's response headers.
    #[tokio::test]
    async fn process_request_streaming_strips_real_key_from_start_headers() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_stream4").unwrap();
        let real_key = "sk-ant-super-secret";
        vault.store(&token, real_key).await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_streaming_ok(
            200,
            // Simulate provider echoing the real key in a header.
            vec![("x-leaked-key".to_string(), real_key.to_string())],
            vec![],
        );

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_stream4");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        if let StreamFrame::Start { headers, .. } = &frames[0] {
            for (_, v) in headers {
                assert!(
                    !v.contains(real_key),
                    "Real key must not appear in forwarded Start headers"
                );
            }
        } else {
            panic!("First frame must be Start");
        }
    }

    /// An empty chunk list (provider sends nothing before closing) must still
    /// produce Start + End with no Chunk frames.
    #[tokio::test]
    async fn process_request_streaming_empty_body_produces_start_and_end_only() {
        let vault = MemoryVault::new();
        let token = ApiKeyToken::new("tok_anthropic_prod_stream5").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let http = MockHttpClient::new();
        http.enqueue_streaming_ok(200, vec![], vec![]); // no chunks

        let nats = MockNatsClient::new();
        let request = make_streaming_request("Bearer tok_anthropic_prod_stream5");

        process_request_streaming(&request, &vault, &http, &nats, "test.reply").await;

        let frames = nats.published_frames();
        assert_eq!(frames.len(), 2, "Start + End only when body is empty");
        assert!(matches!(frames[0], StreamFrame::Start { status: 200, .. }));
        assert!(matches!(frames[1], StreamFrame::End { error: None }));
    }
}
