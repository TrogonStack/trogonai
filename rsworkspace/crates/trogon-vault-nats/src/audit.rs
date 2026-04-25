use std::time::Duration;

use async_nats::jetstream;
use serde::Serialize;

use crate::error::{NatsKvVaultError, NatsResult as _};

pub(crate) const VAULT_AUDIT_STREAM: &str = "VAULT_AUDIT";
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);

// ── AuditEvent ────────────────────────────────────────────────────────────────

/// Structured audit record for every vault operation.
///
/// The real API key (plaintext) is **never** included. Only the proxy token
/// (the `tok_...` identifier) appears in the record.
///
/// Published fire-and-forget to the `VAULT_AUDIT` JetStream stream on
/// subject `vault.audit.{operation}.{vault_name}`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    Store   { token: String, vault: String, actor: String },
    Resolve { token: String, vault: String, success: bool, latency_us: u64 },
    Revoke  { token: String, vault: String, actor: String },
    Rotate  { token: String, vault: String, actor: String },
    /// Published by `trogon-vault-approvals` (Phase 5).
    Approve { proposal_id: String, vault: String, approver: String },
    /// Published by `trogon-vault-approvals` (Phase 5).
    Reject  { proposal_id: String, vault: String, approver: String },
}

impl AuditEvent {
    fn subject(&self, vault_name: &str) -> String {
        let op = match self {
            Self::Store   { .. } => "store",
            Self::Resolve { .. } => "resolve",
            Self::Revoke  { .. } => "revoke",
            Self::Rotate  { .. } => "rotate",
            Self::Approve { .. } => "approve",
            Self::Reject  { .. } => "reject",
        };
        format!("vault.audit.{op}.{vault_name}")
    }
}

// ── AuditPublisher ────────────────────────────────────────────────────────────

/// Fire-and-forget publisher for [`AuditEvent`]s.
///
/// Clones cheaply (Arc-wrapped JetStream context). Each `publish_*` method
/// spawns a background task so the hot path (resolve) is never blocked.
#[derive(Clone)]
pub struct AuditPublisher {
    js:         jetstream::Context,
    vault_name: String,
}

impl AuditPublisher {
    pub fn new(js: jetstream::Context, vault_name: impl Into<String>) -> Self {
        Self { js, vault_name: vault_name.into() }
    }

    pub fn publish_store(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Store {
            token: token.to_string(),
            vault: self.vault_name.clone(),
            actor: actor.to_string(),
        });
    }

    pub fn publish_resolve(&self, token: &str, success: bool, latency_us: u64) {
        self.fire(AuditEvent::Resolve {
            token:      token.to_string(),
            vault:      self.vault_name.clone(),
            success,
            latency_us,
        });
    }

    pub fn publish_revoke(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Revoke {
            token: token.to_string(),
            vault: self.vault_name.clone(),
            actor: actor.to_string(),
        });
    }

    pub fn publish_rotate(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Rotate {
            token: token.to_string(),
            vault: self.vault_name.clone(),
            actor: actor.to_string(),
        });
    }

    /// Used by `trogon-vault-approvals` (Phase 5).
    pub fn publish_approve(&self, proposal_id: &str, approver: &str) {
        self.fire(AuditEvent::Approve {
            proposal_id: proposal_id.to_string(),
            vault:       self.vault_name.clone(),
            approver:    approver.to_string(),
        });
    }

    /// Used by `trogon-vault-approvals` (Phase 5).
    pub fn publish_reject(&self, proposal_id: &str, approver: &str) {
        self.fire(AuditEvent::Reject {
            proposal_id: proposal_id.to_string(),
            vault:       self.vault_name.clone(),
            approver:    approver.to_string(),
        });
    }

    fn fire(&self, event: AuditEvent) {
        let subject = event.subject(&self.vault_name);
        let js = self.js.clone();
        match serde_json::to_vec(&event) {
            Ok(payload) => {
                tokio::spawn(async move {
                    if let Err(e) = js.publish(subject, payload.into()).await {
                        tracing::warn!(error = %e, "audit: failed to publish event");
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "audit: failed to serialize event"),
        }
    }
}

// ── Stream provisioning ───────────────────────────────────────────────────────

/// Create or open the `VAULT_AUDIT` JetStream stream with a 90-day retention window.
///
/// Subjects: `vault.audit.>` — covers all operations and all vault names.
/// Idempotent: safe to call multiple times.
pub async fn ensure_audit_stream(js: &jetstream::Context) -> Result<(), NatsKvVaultError> {
    ensure_audit_stream_with_max_age(js, DEFAULT_MAX_AGE).await
}

/// Like [`ensure_audit_stream`] but with a configurable max-age for the stream.
pub async fn ensure_audit_stream_with_max_age(
    js: &jetstream::Context,
    max_age: Duration,
) -> Result<(), NatsKvVaultError> {
    js.get_or_create_stream(jetstream::stream::Config {
        name:     VAULT_AUDIT_STREAM.to_string(),
        subjects: vec!["vault.audit.>".to_string()],
        storage:  jetstream::stream::StorageType::File,
        max_age,
        ..Default::default()
    })
    .await
    .nats_err()?;
    Ok(())
}
