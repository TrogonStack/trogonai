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

    match forward_request(http_client, request, &forwarded_headers).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, url = %request.url, "Upstream HTTP call failed");
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

    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    use super::resolve_token;

    fn make_headers(auth: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("Authorization".to_string(), auth.to_string());
        m
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
}
