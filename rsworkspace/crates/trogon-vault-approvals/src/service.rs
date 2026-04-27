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
use trogon_vault_nats::AuditPublisher;

use crate::error::ApprovalError;
use crate::notifier::Notifier;
use crate::proposal::{ApproveRequest, CreateRequest, Proposal, ProposalStatus, RejectRequest, StatusResponse};
use crate::subjects::{self, PROPOSALS_STREAM};

// ── ApprovalService ───────────────────────────────────────────────────────────

pub struct ApprovalService<V: VaultStore, N: Notifier> {
    vault:      Arc<V>,
    notifier:   Arc<N>,
    js:         jetstream::Context,
    nats:       async_nats::Client,
    vault_name: String,
    proposals:  Arc<DashMap<String, Proposal>>,
    audit:      Option<Arc<AuditPublisher>>,
}

impl<V, N> ApprovalService<V, N>
where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
    N: Notifier,
{
    pub fn new(
        vault:      Arc<V>,
        notifier:   Arc<N>,
        js:         jetstream::Context,
        nats:       async_nats::Client,
        vault_name: impl Into<String>,
    ) -> Self {
        Self {
            vault,
            notifier,
            js,
            nats,
            vault_name: vault_name.into(),
            proposals:  Arc::new(DashMap::new()),
            audit:      None,
        }
    }

    /// Attach an [`AuditPublisher`] to emit `Approve` / `Reject` audit events.
    pub fn with_audit(mut self, publisher: Arc<AuditPublisher>) -> Self {
        self.audit = Some(publisher);
        self
    }

    /// Run the approval service until the NATS connection is closed.
    ///
    /// Four concurrent tasks:
    /// 1. `consume_creates`    — JetStream pull consumer (durable, at-least-once).
    /// 2. `consume_approvals`  — Core NATS subscriber (ephemeral — plaintext must not persist).
    /// 3. `consume_rejections` — Core NATS subscriber (ephemeral — same reason as approvals).
    /// 4. `serve_status`       — Core NATS subscriber for status request-reply.
    pub async fn run(self) -> Result<(), ApprovalError> {
        let this = Arc::new(self);
        tokio::try_join!(
            Self::consume_creates(Arc::clone(&this)),
            Self::consume_approvals(Arc::clone(&this)),
            Self::consume_rejections(Arc::clone(&this)),
            Self::serve_status(Arc::clone(&this)),
        )?;
        Ok(())
    }

    // ── Consumer helpers ──────────────────────────────────────────────────────

    /// Build (or reopen) a durable pull consumer with the given filter subject.
    async fn make_consumer(
        this:          &Arc<Self>,
        name_suffix:   &str,
        filter_subject: String,
    ) -> Result<jetstream::consumer::Consumer<pull::Config>, ApprovalError> {
        let stream: jetstream::stream::Stream = this.js
            .get_stream(PROPOSALS_STREAM)
            .await
            .map_err(|e: async_nats::error::Error<_>| ApprovalError::JetStream(e.to_string()))?;

        let consumer_name = format!("{}-approvals-{name_suffix}", this.vault_name);

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
            // Parse errors are permanent — retrying won't fix malformed JSON.
            Err(ApprovalError::Serialize(e)) => {
                tracing::warn!(error = %e, "vault-approvals: malformed message, skipping");
                AckKind::Ack
            }
            // Vault / NATS errors may be transient — nack for redelivery.
            Err(e) => {
                tracing::warn!(error = %e, "vault-approvals: transient error, nacking for retry");
                AckKind::Nak(None)
            }
        };
        msg.ack_with(kind)
            .await
            .map_err(|e| ApprovalError::Nats(e.to_string()))
    }

    // ── Three parallel JetStream consumers ────────────────────────────────────

    async fn consume_creates(this: Arc<Self>) -> Result<(), ApprovalError> {
        let filter   = subjects::create(&this.vault_name);
        let consumer = Self::make_consumer(&this, "create", filter).await?;
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
    /// never be stored by the JetStream stream. At-most-once delivery is intentional:
    /// the human can retry if the service is down during the approval.
    async fn consume_approvals(this: Arc<Self>) -> Result<(), ApprovalError> {
        let subject = subjects::approve(&this.vault_name);
        let mut sub = this.nats
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
    async fn consume_rejections(this: Arc<Self>) -> Result<(), ApprovalError> {
        let subject = subjects::reject(&this.vault_name);
        let mut sub = this.nats
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
            status:         ProposalStatus::Pending,
        };

        this.notifier.notify_pending(&proposal, &this.vault_name).await;
        this.proposals.insert(req.id, proposal);
        Ok(())
    }

    async fn handle_approve(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<ApproveRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        let mut entry = match this.proposals.get_mut(&req.proposal_id) {
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

        let token = ApiKeyToken::new(&entry.credential_key)
            .map_err(|e| ApprovalError::Vault(e.to_string()))?;

        this.vault
            .store(&token, &req.plaintext)
            .await
            .map_err(|e| ApprovalError::Vault(e.to_string()))?;

        entry.status = ProposalStatus::Approved { approved_by: req.approved_by.clone() };

        let proposal_snapshot = entry.clone();
        drop(entry);

        // Persist state transition to JetStream for audit and crash recovery.
        publish_state_update(
            &this.js,
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

        // Persist state transition to JetStream for audit and crash recovery.
        publish_state_update(
            &this.js,
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

    async fn serve_status(this: Arc<Self>) -> Result<(), ApprovalError> {
        let subject = subjects::status_wildcard(&this.vault_name);
        let mut sub = this.nats
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

            if let Err(e) = this.nats.publish(reply.to_string(), payload.into()).await {
                tracing::warn!(error = %e, "vault-approvals: failed to publish status response");
            }
        }

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Publish a state-update event to the VAULT_PROPOSALS stream.
///
/// This is fire-and-forget — errors are logged but do not fail the handler.
/// The event persists in JetStream for 90 days (stream retention) and serves
/// as an audit trail and crash-recovery aid for external consumers.
async fn publish_state_update(
    js:          &jetstream::Context,
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
            match js.publish(subject, bytes.into()).await {
                Ok(ack) => { ack.await.ok(); }
                Err(e)  => tracing::warn!(error = %e, "vault-approvals: failed to publish state update"),
            }
        }
        Err(e) => tracing::warn!(error = %e, "vault-approvals: failed to serialize state update"),
    }
}
