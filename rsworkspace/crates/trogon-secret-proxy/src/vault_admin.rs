//! Core NATS request-reply listener for vault administration.
//!
//! Exposes store / rotate / revoke operations over three NATS subjects so that
//! external tools and CI pipelines can manage vault contents at runtime without
//! requiring in-process access.
//!
//! Subjects (relative to a configurable prefix):
//! - `{prefix}.vault.store`  — store a new token → plaintext mapping
//! - `{prefix}.vault.rotate` — update the plaintext for an existing token
//! - `{prefix}.vault.revoke` — remove a token from the vault

use std::sync::Arc;

use async_nats::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use trogon_nats::{PublishClient, SubscribeClient};
use trogon_vault::{ApiKeyToken, VaultStore};

use crate::subjects;

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct VaultStoreRequest {
    pub token: String,
    pub plaintext: String,
}

#[derive(Deserialize)]
pub struct VaultRotateRequest {
    pub token: String,
    pub new_plaintext: String,
}

#[derive(Deserialize)]
pub struct VaultRevokeRequest {
    pub token: String,
}

#[derive(Serialize, Deserialize)]
pub struct VaultAdminResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl VaultAdminResponse {
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
        }
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum VaultAdminError {
    Subscribe { subject: String, source: String },
}

impl std::fmt::Display for VaultAdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe { subject, source } => {
                write!(f, "failed to subscribe to '{subject}': {source}")
            }
        }
    }
}

impl std::error::Error for VaultAdminError {}

// ── Run ───────────────────────────────────────────────────────────────────────

/// Subscribe to vault admin subjects and serve requests until the NATS client
/// is closed or an unrecoverable error occurs.
pub async fn run<N, V>(
    nats: N,
    vault: Arc<V>,
    prefix: &str,
) -> Result<(), VaultAdminError>
where
    N: SubscribeClient + PublishClient,
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
{
    let store_subject = subjects::vault_store(prefix);
    let rotate_subject = subjects::vault_rotate(prefix);
    let revoke_subject = subjects::vault_revoke(prefix);

    let mut store_sub = nats
        .subscribe(store_subject.clone())
        .await
        .map_err(|e| VaultAdminError::Subscribe {
            subject: store_subject.clone(),
            source: e.to_string(),
        })?;

    let mut rotate_sub = nats
        .subscribe(rotate_subject.clone())
        .await
        .map_err(|e| VaultAdminError::Subscribe {
            subject: rotate_subject.clone(),
            source: e.to_string(),
        })?;

    let mut revoke_sub = nats
        .subscribe(revoke_subject.clone())
        .await
        .map_err(|e| VaultAdminError::Subscribe {
            subject: revoke_subject.clone(),
            source: e.to_string(),
        })?;

    tracing::info!(
        store = %store_subject,
        rotate = %rotate_subject,
        revoke = %revoke_subject,
        "Vault admin listener started"
    );

    loop {
        tokio::select! {
            msg = store_sub.next() => {
                let Some(msg) = msg else { break };
                handle_store(&nats, &vault, msg).await;
            }
            msg = rotate_sub.next() => {
                let Some(msg) = msg else { break };
                handle_rotate(&nats, &vault, msg).await;
            }
            msg = revoke_sub.next() => {
                let Some(msg) = msg else { break };
                handle_revoke(&nats, &vault, msg).await;
            }
        }
    }

    Ok(())
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn handle_store<N, V>(nats: &N, vault: &Arc<V>, msg: async_nats::Message)
where
    N: PublishClient,
    V: VaultStore,
    V::Error: std::fmt::Display,
{
    let Some(reply) = msg.reply.clone() else {
        tracing::warn!("vault.store: received message without reply subject, ignoring");
        return;
    };

    let response = match serde_json::from_slice::<VaultStoreRequest>(&msg.payload) {
        Err(e) => VaultAdminResponse::err(format!("Invalid JSON: {e}")),
        Ok(req) => match ApiKeyToken::new(&req.token) {
            Err(e) => VaultAdminResponse::err(format!("Invalid token: {e}")),
            Ok(token) => match vault.store(&token, &req.plaintext).await {
                Ok(()) => VaultAdminResponse::ok(),
                Err(e) => VaultAdminResponse::err(format!("Store failed: {e}")),
            },
        },
    };

    publish_response(nats, reply, &response).await;
}

async fn handle_rotate<N, V>(nats: &N, vault: &Arc<V>, msg: async_nats::Message)
where
    N: PublishClient,
    V: VaultStore,
    V::Error: std::fmt::Display,
{
    let Some(reply) = msg.reply.clone() else {
        tracing::warn!("vault.rotate: received message without reply subject, ignoring");
        return;
    };

    let response = match serde_json::from_slice::<VaultRotateRequest>(&msg.payload) {
        Err(e) => VaultAdminResponse::err(format!("Invalid JSON: {e}")),
        Ok(req) => match ApiKeyToken::new(&req.token) {
            Err(e) => VaultAdminResponse::err(format!("Invalid token: {e}")),
            Ok(token) => match vault.rotate(&token, &req.new_plaintext).await {
                Ok(()) => VaultAdminResponse::ok(),
                Err(e) => VaultAdminResponse::err(format!("Rotate failed: {e}")),
            },
        },
    };

    publish_response(nats, reply, &response).await;
}

async fn handle_revoke<N, V>(nats: &N, vault: &Arc<V>, msg: async_nats::Message)
where
    N: PublishClient,
    V: VaultStore,
    V::Error: std::fmt::Display,
{
    let Some(reply) = msg.reply.clone() else {
        tracing::warn!("vault.revoke: received message without reply subject, ignoring");
        return;
    };

    let response = match serde_json::from_slice::<VaultRevokeRequest>(&msg.payload) {
        Err(e) => VaultAdminResponse::err(format!("Invalid JSON: {e}")),
        Ok(req) => match ApiKeyToken::new(&req.token) {
            Err(e) => VaultAdminResponse::err(format!("Invalid token: {e}")),
            Ok(token) => match vault.revoke(&token).await {
                Ok(()) => VaultAdminResponse::ok(),
                Err(e) => VaultAdminResponse::err(format!("Revoke failed: {e}")),
            },
        },
    };

    publish_response(nats, reply, &response).await;
}

async fn publish_response<N: PublishClient>(
    nats: &N,
    reply: async_nats::Subject,
    response: &VaultAdminResponse,
) {
    let bytes = match serde_json::to_vec(response) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize VaultAdminResponse");
            return;
        }
    };

    if let Err(e) = nats
        .publish_with_headers(reply, HeaderMap::new(), Bytes::from(bytes))
        .await
    {
        tracing::warn!(error = %e, "Failed to publish vault admin response");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_store_request_round_trips() {
        let json = r#"{"token":"tok_anthropic_prod_abc123","plaintext":"sk-ant-key"}"#;
        let req: VaultStoreRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token, "tok_anthropic_prod_abc123");
        assert_eq!(req.plaintext, "sk-ant-key");
    }

    #[test]
    fn vault_rotate_request_round_trips() {
        let json = r#"{"token":"tok_openai_prod_xyz789","new_plaintext":"sk-new-key"}"#;
        let req: VaultRotateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token, "tok_openai_prod_xyz789");
        assert_eq!(req.new_plaintext, "sk-new-key");
    }

    #[test]
    fn vault_revoke_request_round_trips() {
        let json = r#"{"token":"tok_gemini_staging_aabbcc"}"#;
        let req: VaultRevokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token, "tok_gemini_staging_aabbcc");
    }

    #[test]
    fn vault_admin_response_ok_has_no_error_field() {
        let resp = VaultAdminResponse::ok();
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], true);
        assert!(
            v.get("error").is_none(),
            "error key must be absent when None"
        );
    }

    #[test]
    fn vault_admin_response_err_includes_message() {
        let resp = VaultAdminResponse::err("something went wrong");
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "something went wrong");
    }

    // ── VaultAdminError Display ────────────────────────────────────────────────

    #[test]
    fn vault_admin_error_display_includes_subject_and_source() {
        let err = VaultAdminError::Subscribe {
            subject: "trogon.vault.store".to_string(),
            source: "connection refused".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("trogon.vault.store"),
            "display must include subject"
        );
        assert!(
            msg.contains("connection refused"),
            "display must include source error"
        );
    }

    #[test]
    fn vault_admin_error_is_std_error() {
        // Verifies that VaultAdminError implements std::error::Error (compile-time check).
        fn assert_error<E: std::error::Error>() {}
        assert_error::<VaultAdminError>();
    }

    // ── VaultAdminResponse builder edge cases ─────────────────────────────────

    #[test]
    fn vault_admin_response_ok_serializes_without_error_key() {
        let json = serde_json::to_string(&VaultAdminResponse::ok()).unwrap();
        assert!(
            !json.contains("error"),
            "ok response must not contain 'error' key"
        );
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn vault_admin_response_err_with_empty_string_message() {
        let resp = VaultAdminResponse::err("");
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "");
    }

    #[test]
    fn vault_admin_response_err_with_very_long_message() {
        let long_msg = "x".repeat(10_000);
        let resp = VaultAdminResponse::err(&*long_msg);
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["error"].as_str().unwrap().len(), 10_000);
    }

    // ── Request deserialization edge cases ────────────────────────────────────

    #[test]
    fn vault_store_request_rejects_missing_plaintext() {
        let json = r#"{"token":"tok_anthropic_prod_abc123"}"#;
        let result = serde_json::from_str::<VaultStoreRequest>(json);
        assert!(
            result.is_err(),
            "missing plaintext must fail to deserialize"
        );
    }

    #[test]
    fn vault_store_request_rejects_missing_token() {
        let json = r#"{"plaintext":"sk-ant-key"}"#;
        let result = serde_json::from_str::<VaultStoreRequest>(json);
        assert!(result.is_err(), "missing token must fail to deserialize");
    }

    #[test]
    fn vault_rotate_request_rejects_missing_new_plaintext() {
        let json = r#"{"token":"tok_openai_prod_xyz789"}"#;
        let result = serde_json::from_str::<VaultRotateRequest>(json);
        assert!(
            result.is_err(),
            "missing new_plaintext must fail to deserialize"
        );
    }

    #[test]
    fn vault_revoke_request_rejects_missing_token() {
        let result = serde_json::from_str::<VaultRevokeRequest>(r#"{}"#);
        assert!(result.is_err(), "missing token must fail to deserialize");
    }

    #[test]
    fn vault_store_request_rejects_empty_json() {
        let result = serde_json::from_str::<VaultStoreRequest>(r#"{}"#);
        assert!(result.is_err(), "empty JSON must fail to deserialize");
    }

    #[test]
    fn vault_store_request_accepts_extra_fields() {
        // serde ignores unknown fields by default
        let json = r#"{"token":"tok_anthropic_prod_abc","plaintext":"sk-ant","extra":"ignored"}"#;
        let req = serde_json::from_str::<VaultStoreRequest>(json).unwrap();
        assert_eq!(req.token, "tok_anthropic_prod_abc");
    }

    // ── Handler tests using mocks ─────────────────────────────────────────────

    use std::sync::Arc;
    use trogon_nats::MockNatsClient;
    use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

    fn make_msg(payload: &[u8], reply: &str) -> async_nats::Message {
        let reply_bytes = bytes::Bytes::from(reply.to_string());
        async_nats::Message {
            subject: "vault.store".into(),
            reply: Some(async_nats::Subject::from_utf8(reply_bytes).unwrap()),
            payload: bytes::Bytes::from(payload.to_vec()),
            headers: None,
            length: payload.len(),
            status: None,
            description: None,
        }
    }

    #[tokio::test]
    async fn handle_store_stores_token_and_publishes_ok() {
        let nats = MockNatsClient::new();
        let vault = Arc::new(MemoryVault::new());

        let payload = serde_json::to_vec(&serde_json::json!({
            "token": "tok_anthropic_prod_abc123",
            "plaintext": "sk-ant-realkey"
        }))
        .unwrap();
        let msg = make_msg(&payload, "reply.store.1");

        handle_store(&nats, &vault, msg).await;

        // Verify the token was stored in the vault.
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        let resolved = vault.resolve(&token).await.unwrap();
        assert_eq!(resolved.as_deref(), Some("sk-ant-realkey"));

        // Verify a reply was published.
        let published = nats.published_payloads();
        assert_eq!(published.len(), 1);
        let resp: VaultAdminResponse = serde_json::from_slice(&published[0]).unwrap();
        assert!(resp.ok);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn handle_store_invalid_json_publishes_error() {
        let nats = MockNatsClient::new();
        let vault = Arc::new(MemoryVault::new());

        let msg = make_msg(b"not valid json", "reply.store.2");
        handle_store(&nats, &vault, msg).await;

        let published = nats.published_payloads();
        assert_eq!(published.len(), 1);
        let resp: VaultAdminResponse = serde_json::from_slice(&published[0]).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn handle_revoke_removes_token_and_publishes_ok() {
        let nats = MockNatsClient::new();
        let vault = Arc::new(MemoryVault::new());

        // Pre-store a token.
        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, "sk-ant-realkey").await.unwrap();

        let payload = serde_json::to_vec(&serde_json::json!({
            "token": "tok_anthropic_prod_abc123"
        }))
        .unwrap();
        let msg = make_msg(&payload, "reply.revoke.1");

        handle_revoke(&nats, &vault, msg).await;

        let resolved = vault.resolve(&token).await.unwrap();
        assert!(resolved.is_none(), "token must be gone after revoke");

        let published = nats.published_payloads();
        assert_eq!(published.len(), 1);
        let resp: VaultAdminResponse = serde_json::from_slice(&published[0]).unwrap();
        assert!(resp.ok);
    }

    #[tokio::test]
    async fn handle_rotate_updates_token_and_publishes_ok() {
        let nats = MockNatsClient::new();
        let vault = Arc::new(MemoryVault::new());

        let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();
        vault.store(&token, "sk-ant-old").await.unwrap();

        let payload = serde_json::to_vec(&serde_json::json!({
            "token": "tok_anthropic_prod_abc123",
            "new_plaintext": "sk-ant-new"
        }))
        .unwrap();
        let msg = make_msg(&payload, "reply.rotate.1");

        handle_rotate(&nats, &vault, msg).await;

        let resolved = vault.resolve(&token).await.unwrap();
        assert_eq!(resolved.as_deref(), Some("sk-ant-new"));

        let published = nats.published_payloads();
        assert_eq!(published.len(), 1);
        let resp: VaultAdminResponse = serde_json::from_slice(&published[0]).unwrap();
        assert!(resp.ok);
    }

    #[tokio::test]
    async fn handle_store_no_reply_subject_does_not_publish() {
        let nats = MockNatsClient::new();
        let vault = Arc::new(MemoryVault::new());

        // Message without a reply subject — handler should silently ignore.
        let payload = serde_json::to_vec(&serde_json::json!({
            "token": "tok_anthropic_prod_abc123",
            "plaintext": "sk-ant-key"
        }))
        .unwrap();
        let msg = async_nats::Message {
            subject: "vault.store".into(),
            reply: None,
            payload: bytes::Bytes::from(payload),
            headers: None,
            length: 0,
            status: None,
            description: None,
        };

        handle_store(&nats, &vault, msg).await;
        assert!(nats.published_payloads().is_empty());
    }
}
