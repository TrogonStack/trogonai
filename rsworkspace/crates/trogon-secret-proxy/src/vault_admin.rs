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

use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
pub struct VaultAdminResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl VaultAdminResponse {
    pub fn ok() -> Self {
        Self { ok: true, error: None }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()) }
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
pub async fn run<V>(
    nats: async_nats::Client,
    vault: Arc<V>,
    prefix: &str,
) -> Result<(), VaultAdminError>
where
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

    use futures_util::StreamExt as _;

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

async fn handle_store<V>(nats: &async_nats::Client, vault: &Arc<V>, msg: async_nats::Message)
where
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

async fn handle_rotate<V>(nats: &async_nats::Client, vault: &Arc<V>, msg: async_nats::Message)
where
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

async fn handle_revoke<V>(nats: &async_nats::Client, vault: &Arc<V>, msg: async_nats::Message)
where
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

async fn publish_response(
    nats: &async_nats::Client,
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

    if let Err(e) = nats.publish(reply, bytes.into()).await {
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
        assert!(v.get("error").is_none(), "error key must be absent when None");
    }

    #[test]
    fn vault_admin_response_err_includes_message() {
        let resp = VaultAdminResponse::err("something went wrong");
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "something went wrong");
    }
}
