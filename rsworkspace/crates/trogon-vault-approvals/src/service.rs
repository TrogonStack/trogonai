//! Human-in-the-loop approval service for vault credential writes.
//!
//! Three parallel JetStream pull consumers handle create / approve / reject events
//! independently. A fourth core-NATS subscriber serves status request-reply.
//!
//! Run with [`ApprovalService::run`] — the method loops indefinitely until the
//! NATS connection is closed.

use std::sync::Arc;

use async_nats::jetstream;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use async_nats::jetstream::AckKind;
use dashmap::DashMap;
use futures_util::StreamExt as _;
use trogon_vault::{ApiKeyToken, VaultStore};
use trogon_vault_nats::Audit;

use crate::error::ApprovalError;
use crate::notifier::Notifier;
use crate::proposal::{ApproveRequest, CreateRequest, Proposal, ProposalStatus, RejectRequest, StatusResponse};
use crate::subjects::{self, PROPOSALS_STREAM};

// ── StatePublisher trait ──────────────────────────────────────────────────────

/// Abstraction over fire-and-forget state-update publishing to the VAULT_PROPOSALS stream.
///
/// [`JetStreamStatePublisher`] is the real implementation.
/// [`NoopStatePublisher`] is used in tests where persistence is not under test.
#[async_trait::async_trait]
pub trait StatePublisher: Send + Sync + 'static {
    async fn publish_state_update(
        &self,
        vault_name:  &str,
        proposal_id: &str,
        state:       &str,
        by:          &str,
        reason:      Option<&str>,
    );
}

// ── JetStreamStatePublisher ───────────────────────────────────────────────────

/// Publishes state-update events to JetStream for audit and crash recovery.
pub struct JetStreamStatePublisher {
    js: jetstream::Context,
}

impl JetStreamStatePublisher {
    pub fn new(js: jetstream::Context) -> Self {
        Self { js }
    }
}

#[async_trait::async_trait]
impl StatePublisher for JetStreamStatePublisher {
    async fn publish_state_update(
        &self,
        vault_name:  &str,
        proposal_id: &str,
        state:       &str,
        by:          &str,
        reason:      Option<&str>,
    ) {
        let mut payload = serde_json::json!({
            "proposal_id": proposal_id,
            "vault":       vault_name,
            "state":       state,
            "by":          by,
        });
        if let Some(r) = reason {
            payload["reason"] = serde_json::Value::String(r.to_string());
        }
        let subject = subjects::state_update(vault_name, proposal_id);
        match serde_json::to_vec(&payload) {
            Ok(bytes) => {
                match self.js.publish(subject, bytes.into()).await {
                    Ok(ack) => { ack.await.ok(); }
                    Err(e)  => tracing::warn!(error = %e, "vault-approvals: failed to publish state update"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "vault-approvals: failed to serialize state update"),
        }
    }
}

// ── NoopStatePublisher ────────────────────────────────────────────────────────

/// Discards all state-update events — for tests.
pub struct NoopStatePublisher;

#[async_trait::async_trait]
impl StatePublisher for NoopStatePublisher {
    async fn publish_state_update(&self, _: &str, _: &str, _: &str, _: &str, _: Option<&str>) {}
}

// ── ApprovalService ───────────────────────────────────────────────────────────

pub struct ApprovalService<V: VaultStore, N: Notifier, P: StatePublisher> {
    vault:      Arc<V>,
    notifier:   Arc<N>,
    publisher:  Arc<P>,
    vault_name: String,
    proposals:  Arc<DashMap<String, Proposal>>,
    audit:      Option<Arc<dyn Audit>>,
}

impl<V, N, P> ApprovalService<V, N, P>
where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
    N: Notifier,
    P: StatePublisher,
{
    pub fn new(
        vault:      Arc<V>,
        notifier:   Arc<N>,
        publisher:  Arc<P>,
        vault_name: impl Into<String>,
    ) -> Self {
        Self {
            vault,
            notifier,
            publisher,
            vault_name: vault_name.into(),
            proposals:  Arc::new(DashMap::new()),
            audit:      None,
        }
    }

    /// Attach an [`Audit`] implementation to emit audit events on approve / reject.
    pub fn with_audit(mut self, audit: Arc<dyn Audit>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Run the approval service until the NATS connection is closed.
    ///
    /// Four concurrent tasks:
    /// 1. `consume_creates`    — JetStream pull consumer (durable, at-least-once).
    /// 2. `consume_approvals`  — Core NATS subscriber (ephemeral — plaintext must not persist).
    /// 3. `consume_rejections` — Core NATS subscriber (ephemeral — same reason as approvals).
    /// 4. `serve_status`       — Core NATS subscriber for status request-reply.
    pub async fn run(
        self,
        js:   jetstream::Context,
        nats: async_nats::Client,
    ) -> Result<(), ApprovalError> {
        let this = Arc::new(self);
        tokio::try_join!(
            Self::consume_creates(Arc::clone(&this), js),
            Self::consume_approvals(Arc::clone(&this), nats.clone()),
            Self::consume_rejections(Arc::clone(&this), nats.clone()),
            Self::serve_status(Arc::clone(&this), nats),
        )?;
        Ok(())
    }

    // ── Consumer helpers ──────────────────────────────────────────────────────

    async fn make_consumer(
        js:             &jetstream::Context,
        vault_name:     &str,
        name_suffix:    &str,
        filter_subject: String,
    ) -> Result<jetstream::consumer::Consumer<pull::Config>, ApprovalError> {
        let stream: jetstream::stream::Stream = js
            .get_stream(PROPOSALS_STREAM)
            .await
            .map_err(|e: async_nats::error::Error<_>| ApprovalError::JetStream(e.to_string()))?;

        let consumer_name = format!("{vault_name}-approvals-{name_suffix}");

        stream
            .get_or_create_consumer(
                &consumer_name,
                pull::Config {
                    durable_name:   Some(consumer_name.clone()),
                    filter_subject,
                    ack_policy:     AckPolicy::Explicit,
                    deliver_policy: DeliverPolicy::All,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| ApprovalError::JetStream(e.to_string()))
    }

    /// Ack on success, nack on transient errors, ack (skip) on parse errors.
    async fn settle(
        msg:    jetstream::Message,
        result: Result<(), ApprovalError>,
    ) -> Result<(), ApprovalError> {
        let kind = match &result {
            Ok(()) => AckKind::Ack,
            Err(ApprovalError::Serialize(e)) => {
                tracing::warn!(error = %e, "vault-approvals: malformed message, skipping");
                AckKind::Ack
            }
            Err(e) => {
                tracing::warn!(error = %e, "vault-approvals: transient error, nacking for retry");
                AckKind::Nak(None)
            }
        };
        msg.ack_with(kind)
            .await
            .map_err(|e| ApprovalError::Nats(e.to_string()))
    }

    // ── Three parallel consumers ──────────────────────────────────────────────

    async fn consume_creates(this: Arc<Self>, js: jetstream::Context) -> Result<(), ApprovalError> {
        let filter   = subjects::create(&this.vault_name);
        let consumer = Self::make_consumer(&js, &this.vault_name, "create", filter).await?;
        let mut msgs = consumer.messages().await
            .map_err(|e| ApprovalError::JetStream(e.to_string()))?;

        while let Some(msg) = msgs.next().await {
            let msg    = msg.map_err(|e| ApprovalError::JetStream(e.to_string()))?;
            let result = Self::handle_create(&this, &msg.payload).await;
            Self::settle(msg, result).await?;
        }
        Ok(())
    }

    /// Core NATS subscription — approve messages carry plaintext API keys and must
    /// never be stored by the JetStream stream. At-most-once delivery is intentional.
    async fn consume_approvals(this: Arc<Self>, nats: async_nats::Client) -> Result<(), ApprovalError> {
        let subject = subjects::approve(&this.vault_name);
        let mut sub = nats
            .subscribe(subject)
            .await
            .map_err(|e| ApprovalError::Nats(e.to_string()))?;

        while let Some(msg) = sub.next().await {
            if let Err(e) = Self::handle_approve(&this, &msg.payload).await {
                tracing::warn!(error = %e, "vault-approvals: error handling approve");
            }
        }
        Ok(())
    }

    /// Core NATS subscription — same plaintext-safety rationale as consume_approvals.
    async fn consume_rejections(this: Arc<Self>, nats: async_nats::Client) -> Result<(), ApprovalError> {
        let subject = subjects::reject(&this.vault_name);
        let mut sub = nats
            .subscribe(subject)
            .await
            .map_err(|e| ApprovalError::Nats(e.to_string()))?;

        while let Some(msg) = sub.next().await {
            if let Err(e) = Self::handle_reject(&this, &msg.payload).await {
                tracing::warn!(error = %e, "vault-approvals: error handling reject");
            }
        }
        Ok(())
    }

    // ── Event handlers ────────────────────────────────────────────────────────

    async fn handle_create(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<CreateRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        if this.proposals.contains_key(&req.id) {
            tracing::debug!(id = %req.id, "vault-approvals: duplicate create, skipping");
            return Ok(());
        }

        let proposal = Proposal {
            id:             req.id.clone(),
            credential_key: req.credential_key,
            service:        req.service,
            message:        req.message,
            requested_at:   req.requested_at,
            status:         ProposalStatus::Pending,
        };

        this.notifier.notify_pending(&proposal, &this.vault_name).await;
        this.proposals.insert(req.id, proposal);
        Ok(())
    }

    async fn handle_approve(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<ApproveRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        // Extract what we need and release the lock BEFORE any async IO.
        // DashMap's RefMut holds a write-lock on its shard; keeping it across
        // an await blocks serve_status's concurrent proposals.get() reads.
        let credential_key = {
            let entry = match this.proposals.get(&req.proposal_id) {
                Some(e) => e,
                None => {
                    tracing::warn!(
                        proposal_id = %req.proposal_id,
                        "vault-approvals: approve for unknown proposal, ignoring"
                    );
                    return Ok(());
                }
            };
            if !matches!(entry.status, ProposalStatus::Pending) {
                tracing::warn!(
                    proposal_id = %req.proposal_id,
                    "vault-approvals: approve for non-pending proposal, ignoring"
                );
                return Ok(());
            }
            entry.credential_key.clone()
        }; // read lock released here

        let token = ApiKeyToken::new(&credential_key)
            .map_err(|e| ApprovalError::Vault(e.to_string()))?;

        this.vault
            .store(&token, &req.plaintext)
            .await
            .map_err(|e| ApprovalError::Vault(e.to_string()))?;

        // Re-acquire as write to update status; guard against concurrent approvals.
        let mut entry = match this.proposals.get_mut(&req.proposal_id) {
            Some(e) => e,
            None    => return Ok(()),
        };
        if !matches!(entry.status, ProposalStatus::Pending) {
            return Ok(());
        }
        entry.status = ProposalStatus::Approved { approved_by: req.approved_by.clone() };

        let proposal_snapshot = entry.clone();
        drop(entry);

        this.publisher.publish_state_update(
            &this.vault_name,
            &req.proposal_id,
            "approved",
            &req.approved_by,
            None,
        ).await;

        this.notifier.notify_approved(&proposal_snapshot, &req.approved_by).await;

        if let Some(ref audit) = this.audit {
            audit.publish_approve(&req.proposal_id, &req.approved_by);
        }

        Ok(())
    }

    async fn handle_reject(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<RejectRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        let mut entry = match this.proposals.get_mut(&req.proposal_id) {
            Some(e) => e,
            None => {
                tracing::warn!(
                    proposal_id = %req.proposal_id,
                    "vault-approvals: reject for unknown proposal, ignoring"
                );
                return Ok(());
            }
        };

        if !matches!(entry.status, ProposalStatus::Pending) {
            tracing::warn!(
                proposal_id = %req.proposal_id,
                "vault-approvals: reject for non-pending proposal, ignoring"
            );
            return Ok(());
        }

        entry.status = ProposalStatus::Rejected {
            rejected_by: req.rejected_by.clone(),
            reason:      req.reason.clone(),
        };

        let proposal_snapshot = entry.clone();
        drop(entry);

        this.publisher.publish_state_update(
            &this.vault_name,
            &req.proposal_id,
            "rejected",
            &req.rejected_by,
            Some(&req.reason),
        ).await;

        this.notifier.notify_rejected(&proposal_snapshot, &req.rejected_by, &req.reason).await;

        if let Some(ref audit) = this.audit {
            audit.publish_reject(&req.proposal_id, &req.rejected_by);
        }

        Ok(())
    }

    // ── Status request-reply ──────────────────────────────────────────────────

    async fn serve_status(this: Arc<Self>, nats: async_nats::Client) -> Result<(), ApprovalError> {
        let subject = subjects::status_wildcard(&this.vault_name);
        let mut sub = nats
            .subscribe(subject)
            .await
            .map_err(|e| ApprovalError::Nats(e.to_string()))?;

        let prefix = format!("vault.proposals.{}.status.", this.vault_name);

        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply.clone() else { continue; };

            let proposal_id = msg.subject
                .as_str()
                .strip_prefix(&prefix)
                .unwrap_or("")
                .to_string();

            let response = match this.proposals.get(&proposal_id) {
                None    => StatusResponse::not_found(proposal_id),
                Some(p) => StatusResponse::from_proposal(&*p),
            };

            let payload = match serde_json::to_vec(&response) {
                Ok(b)  => b,
                Err(e) => {
                    tracing::error!(error = %e, "vault-approvals: failed to serialize status response");
                    continue;
                }
            };

            if let Err(e) = nats.publish(reply.to_string(), payload.into()).await {
                tracing::warn!(error = %e, "vault-approvals: failed to publish status response");
            }
        }

        Ok(())
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_vault::MemoryVault;

    use crate::notifier::NoopNotifier;

    fn service() -> ApprovalService<MemoryVault, NoopNotifier, NoopStatePublisher> {
        ApprovalService::new(
            Arc::new(MemoryVault::new()),
            Arc::new(NoopNotifier),
            Arc::new(NoopStatePublisher),
            "prod",
        )
    }

    fn arc_service() -> Arc<ApprovalService<MemoryVault, NoopNotifier, NoopStatePublisher>> {
        Arc::new(service())
    }

    // ── handle_create ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_inserts_pending_proposal() {
        let svc = arc_service();
        let payload = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"api.stripe.com","message":"need access"}"#;

        ApprovalService::handle_create(&svc, payload).await.unwrap();

        let p = svc.proposals.get("prop_abc").expect("proposal must be in map");
        assert!(matches!(p.status, ProposalStatus::Pending));
        assert_eq!(p.service, "api.stripe.com");
    }

    #[tokio::test]
    async fn create_duplicate_is_idempotent() {
        let svc = arc_service();
        let payload = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"api.stripe.com","message":"x"}"#;

        ApprovalService::handle_create(&svc, payload).await.unwrap();
        ApprovalService::handle_create(&svc, payload).await.unwrap();

        assert_eq!(svc.proposals.len(), 1);
    }

    #[tokio::test]
    async fn create_with_requested_at_preserves_timestamp() {
        let svc = arc_service();
        let payload = br#"{"id":"prop_ts","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m","requested_at":"2026-04-27T10:00:00Z"}"#;

        ApprovalService::handle_create(&svc, payload).await.unwrap();

        let p = svc.proposals.get("prop_ts").unwrap();
        assert_eq!(p.requested_at.as_deref(), Some("2026-04-27T10:00:00Z"));
    }

    #[tokio::test]
    async fn create_returns_error_on_malformed_json() {
        let svc = arc_service();
        let result = ApprovalService::handle_create(&svc, b"not json").await;
        assert!(matches!(result, Err(ApprovalError::Serialize(_))));
    }

    // ── handle_approve ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn approve_stores_credential_and_updates_status() {
        let svc = arc_service();

        // Create proposal first
        let create_payload = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m"}"#;
        ApprovalService::handle_create(&svc, create_payload).await.unwrap();

        let approve_payload = br#"{"proposal_id":"prop_abc","approved_by":"mario","plaintext":"sk_live_secret"}"#;
        ApprovalService::handle_approve(&svc, approve_payload).await.unwrap();

        let p = svc.proposals.get("prop_abc").unwrap();
        assert!(matches!(&p.status, ProposalStatus::Approved { approved_by } if approved_by == "mario"));

        // Verify the vault actually stored the credential
        let token = ApiKeyToken::new("tok_stripe_prod_abc1").unwrap();
        let stored = svc.vault.resolve(&token).await.unwrap();
        assert_eq!(stored.as_deref(), Some("sk_live_secret"));
    }

    #[tokio::test]
    async fn approve_unknown_proposal_is_ignored() {
        let svc = arc_service();
        let payload = br#"{"proposal_id":"prop_unknown","approved_by":"mario","plaintext":"sk_live_x"}"#;
        assert!(ApprovalService::handle_approve(&svc, payload).await.is_ok());
        assert!(svc.proposals.is_empty());
    }

    #[tokio::test]
    async fn approve_non_pending_proposal_is_ignored() {
        let svc = arc_service();

        let create  = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m"}"#;
        let approve = br#"{"proposal_id":"prop_abc","approved_by":"mario","plaintext":"sk_live_x"}"#;
        ApprovalService::handle_create(&svc, create).await.unwrap();
        ApprovalService::handle_approve(&svc, approve).await.unwrap();

        // Second approve on already-approved proposal must be a no-op
        let second = br#"{"proposal_id":"prop_abc","approved_by":"luigi","plaintext":"sk_live_y"}"#;
        ApprovalService::handle_approve(&svc, second).await.unwrap();

        let p = svc.proposals.get("prop_abc").unwrap();
        assert!(matches!(&p.status, ProposalStatus::Approved { approved_by } if approved_by == "mario"),
            "second approve must not overwrite first");
    }

    // ── handle_reject ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reject_updates_status_with_reason() {
        let svc = arc_service();

        let create  = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m"}"#;
        let reject  = br#"{"proposal_id":"prop_abc","rejected_by":"luigi","reason":"not authorised"}"#;
        ApprovalService::handle_create(&svc, create).await.unwrap();
        ApprovalService::handle_reject(&svc, reject).await.unwrap();

        let p = svc.proposals.get("prop_abc").unwrap();
        assert!(matches!(
            &p.status,
            ProposalStatus::Rejected { rejected_by, reason }
            if rejected_by == "luigi" && reason == "not authorised"
        ));
    }

    #[tokio::test]
    async fn reject_unknown_proposal_is_ignored() {
        let svc = arc_service();
        let payload = br#"{"proposal_id":"prop_unknown","rejected_by":"luigi","reason":"nope"}"#;
        assert!(ApprovalService::handle_reject(&svc, payload).await.is_ok());
    }

    #[tokio::test]
    async fn reject_non_pending_proposal_is_ignored() {
        let svc = arc_service();

        let create  = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m"}"#;
        let reject1 = br#"{"proposal_id":"prop_abc","rejected_by":"luigi","reason":"first"}"#;
        let reject2 = br#"{"proposal_id":"prop_abc","rejected_by":"mario","reason":"second"}"#;
        ApprovalService::handle_create(&svc, create).await.unwrap();
        ApprovalService::handle_reject(&svc, reject1).await.unwrap();
        ApprovalService::handle_reject(&svc, reject2).await.unwrap();

        let p = svc.proposals.get("prop_abc").unwrap();
        assert!(matches!(
            &p.status,
            ProposalStatus::Rejected { rejected_by, .. } if rejected_by == "luigi"
        ), "second reject must not overwrite first");
    }

    #[tokio::test]
    async fn reject_without_reason_defaults_to_empty_string() {
        let svc = arc_service();

        let create = br#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"s","message":"m"}"#;
        let reject = br#"{"proposal_id":"prop_abc","rejected_by":"luigi"}"#;
        ApprovalService::handle_create(&svc, create).await.unwrap();
        ApprovalService::handle_reject(&svc, reject).await.unwrap();

        let p = svc.proposals.get("prop_abc").unwrap();
        assert!(matches!(
            &p.status,
            ProposalStatus::Rejected { reason, .. } if reason.is_empty()
        ));
    }
}
