use std::time::Duration;

use async_nats::jetstream;
use serde::Serialize;

use crate::error::{NatsKvVaultError, NatsResult as _};

pub(crate) const VAULT_AUDIT_STREAM: &str = "VAULT_AUDIT";

// ── Audit trait ───────────────────────────────────────────────────────────────

/// Abstraction over audit event publication.
///
/// [`AuditPublisher`] is the real implementation (fire-and-forget JetStream).
/// [`NoopAudit`] is used in tests and deployments that do not need audit logs.
pub trait Audit: Send + Sync + 'static {
    fn publish_store(&self, token: &str, actor: &str);
    fn publish_resolve(&self, token: &str, success: bool, latency_us: u64);
    fn publish_revoke(&self, token: &str, actor: &str);
    fn publish_rotate(&self, token: &str, actor: &str);
    fn publish_approve(&self, proposal_id: &str, approver: &str);
    fn publish_reject(&self, proposal_id: &str, approver: &str);
}

// ── NoopAudit ─────────────────────────────────────────────────────────────────

/// Discards every audit event — for tests and deployments without audit logging.
pub struct NoopAudit;

impl Audit for NoopAudit {
    fn publish_store(&self, _: &str, _: &str) {}
    fn publish_resolve(&self, _: &str, _: bool, _: u64) {}
    fn publish_revoke(&self, _: &str, _: &str) {}
    fn publish_rotate(&self, _: &str, _: &str) {}
    fn publish_approve(&self, _: &str, _: &str) {}
    fn publish_reject(&self, _: &str, _: &str) {}
}
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

impl Audit for AuditPublisher {
    fn publish_store(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Store { token: token.into(), vault: self.vault_name.clone(), actor: actor.into() });
    }
    fn publish_resolve(&self, token: &str, success: bool, latency_us: u64) {
        self.fire(AuditEvent::Resolve { token: token.into(), vault: self.vault_name.clone(), success, latency_us });
    }
    fn publish_revoke(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Revoke { token: token.into(), vault: self.vault_name.clone(), actor: actor.into() });
    }
    fn publish_rotate(&self, token: &str, actor: &str) {
        self.fire(AuditEvent::Rotate { token: token.into(), vault: self.vault_name.clone(), actor: actor.into() });
    }
    fn publish_approve(&self, proposal_id: &str, approver: &str) {
        self.fire(AuditEvent::Approve { proposal_id: proposal_id.into(), vault: self.vault_name.clone(), approver: approver.into() });
    }
    fn publish_reject(&self, proposal_id: &str, approver: &str) {
        self.fire(AuditEvent::Reject { proposal_id: proposal_id.into(), vault: self.vault_name.clone(), approver: approver.into() });
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── AuditEvent serialization ──────────────────────────────────────────────

    #[test]
    fn store_event_serializes_type_tag() {
        let event = AuditEvent::Store {
            token: "tok_stripe_prod_abc".into(),
            vault: "prod".into(),
            actor: "mario".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "store");
        assert_eq!(v["token"], "tok_stripe_prod_abc");
        assert_eq!(v["vault"], "prod");
        assert_eq!(v["actor"], "mario");
    }

    #[test]
    fn resolve_event_serializes_success_and_latency() {
        let event = AuditEvent::Resolve {
            token:      "tok_openai_staging_xyz".into(),
            vault:      "staging".into(),
            success:    true,
            latency_us: 42,
        };
        let v: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "resolve");
        assert_eq!(v["success"], true);
        assert_eq!(v["latency_us"], 42);
    }

    #[test]
    fn approve_event_serializes_proposal_fields() {
        let event = AuditEvent::Approve {
            proposal_id: "prop_abc123".into(),
            vault:       "prod".into(),
            approver:    "luigi".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "approve");
        assert_eq!(v["proposal_id"], "prop_abc123");
        assert_eq!(v["approver"], "luigi");
    }

    #[test]
    fn reject_event_serializes_proposal_fields() {
        let event = AuditEvent::Reject {
            proposal_id: "prop_xyz".into(),
            vault:       "prod".into(),
            approver:    "peach".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "reject");
        assert_eq!(v["proposal_id"], "prop_xyz");
    }

    #[test]
    fn no_audit_event_contains_plaintext_field() {
        let events = vec![
            serde_json::to_value(AuditEvent::Store   { token: "t".into(), vault: "v".into(), actor: "a".into() }).unwrap(),
            serde_json::to_value(AuditEvent::Resolve { token: "t".into(), vault: "v".into(), success: true, latency_us: 0 }).unwrap(),
            serde_json::to_value(AuditEvent::Revoke  { token: "t".into(), vault: "v".into(), actor: "a".into() }).unwrap(),
            serde_json::to_value(AuditEvent::Rotate  { token: "t".into(), vault: "v".into(), actor: "a".into() }).unwrap(),
            serde_json::to_value(AuditEvent::Approve { proposal_id: "p".into(), vault: "v".into(), approver: "a".into() }).unwrap(),
            serde_json::to_value(AuditEvent::Reject  { proposal_id: "p".into(), vault: "v".into(), approver: "a".into() }).unwrap(),
        ];
        for v in &events {
            let s = v.to_string();
            assert!(!s.contains("\"plaintext\""), "audit event must never contain plaintext: {s}");
            assert!(!s.contains("\"value\""),     "audit event must never contain value: {s}");
            assert!(!s.contains("\"secret\""),    "audit event must never contain secret: {s}");
        }
    }

    // ── AuditEvent::subject ───────────────────────────────────────────────────

    #[test]
    fn subject_format_for_each_operation() {
        let cases: &[(&AuditEvent, &str)] = &[
            (&AuditEvent::Store   { token: "t".into(), vault: "v".into(), actor: "a".into()                     }, "vault.audit.store.prod"),
            (&AuditEvent::Resolve { token: "t".into(), vault: "v".into(), success: true, latency_us: 0          }, "vault.audit.resolve.prod"),
            (&AuditEvent::Revoke  { token: "t".into(), vault: "v".into(), actor: "a".into()                     }, "vault.audit.revoke.prod"),
            (&AuditEvent::Rotate  { token: "t".into(), vault: "v".into(), actor: "a".into()                     }, "vault.audit.rotate.prod"),
            (&AuditEvent::Approve { proposal_id: "p".into(), vault: "v".into(), approver: "a".into()            }, "vault.audit.approve.prod"),
            (&AuditEvent::Reject  { proposal_id: "p".into(), vault: "v".into(), approver: "a".into()            }, "vault.audit.reject.prod"),
        ];
        for (event, expected) in cases {
            assert_eq!(event.subject("prod"), *expected);
        }
    }
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
