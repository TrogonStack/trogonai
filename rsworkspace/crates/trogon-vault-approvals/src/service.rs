//! Human-in-the-loop approval service for vault credential writes.
//!
//! Subscribes to `vault.proposals.{vault_name}.create/approve/reject` via a
//! durable JetStream pull consumer and serves status queries via core NATS
//! request-reply.
//!
//! Run with [`ApprovalService::run`] — the method loops indefinitely until the
//! NATS connection is closed.

use std::sync::Arc;

use async_nats::jetstream;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
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
    /// Two concurrent loops:
    /// 1. JetStream pull consumer — processes create / approve / reject events.
    /// 2. Core NATS subscriber — serves status request-reply.
    pub async fn run(self) -> Result<(), ApprovalError> {
        let this = Arc::new(self);
        tokio::try_join!(
            Self::consume_events(Arc::clone(&this)),
            Self::serve_status(Arc::clone(&this)),
        )?;
        Ok(())
    }

    // ── JetStream consumer ────────────────────────────────────────────────────

    async fn consume_events(this: Arc<Self>) -> Result<(), ApprovalError> {
        let stream = this.js
            .get_stream(PROPOSALS_STREAM)
            .await
            .map_err(|e| ApprovalError::JetStream(e.to_string()))?;

        let consumer_name = format!("vault-approvals-{}", this.vault_name);
        let filter = subjects::vault_wildcard(&this.vault_name);

        let consumer: jetstream::consumer::Consumer<pull::Config> = stream
            .get_or_create_consumer(
                &consumer_name,
                pull::Config {
                    durable_name:   Some(consumer_name.clone()),
                    filter_subject: filter,
                    ack_policy:     AckPolicy::Explicit,
                    deliver_policy: DeliverPolicy::All,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| ApprovalError::JetStream(e.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| ApprovalError::JetStream(e.to_string()))?;

        while let Some(msg) = messages.next().await {
            let msg = msg.map_err(|e| ApprovalError::JetStream(e.to_string()))?;
            let subject = msg.subject.to_string();

            let result = if subject.ends_with(".create") {
                Self::handle_create(&this, &msg.payload).await
            } else if subject.ends_with(".approve") {
                Self::handle_approve(&this, &msg.payload).await
            } else if subject.ends_with(".reject") {
                Self::handle_reject(&this, &msg.payload).await
            } else {
                tracing::warn!(subject = %subject, "vault-approvals: unknown subject, skipping");
                Ok(())
            };

            match result {
                Ok(()) => {
                    msg.ack().await.map_err(|e| ApprovalError::Nats(e.to_string()))?;
                }
                Err(e) => {
                    tracing::warn!(error = %e, subject = %subject, "vault-approvals: error processing event, nacking");
                    msg.ack_with(async_nats::jetstream::AckKind::Nak(None))
                        .await
                        .map_err(|e| ApprovalError::Nats(e.to_string()))?;
                }
            }
        }

        Ok(())
    }

    // ── Event handlers ────────────────────────────────────────────────────────

    async fn handle_create(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<CreateRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        let proposal = Proposal {
            id:             req.id.clone(),
            credential_key: req.credential_key,
            service:        req.service,
            message:        req.message,
            status:         ProposalStatus::Pending,
        };

        // Idempotent: if already exists, skip to avoid overwriting a later state.
        if this.proposals.contains_key(&req.id) {
            tracing::debug!(id = %req.id, "vault-approvals: duplicate create, skipping");
            return Ok(());
        }

        this.notifier.notify_pending(&proposal).await;
        this.proposals.insert(req.id, proposal);
        Ok(())
    }

    async fn handle_approve(this: &Arc<Self>, payload: &[u8]) -> Result<(), ApprovalError> {
        let req = serde_json::from_slice::<ApproveRequest>(payload)
            .map_err(|e| ApprovalError::Serialize(e.to_string()))?;

        let mut entry = match this.proposals.get_mut(&req.proposal_id) {
            Some(e) => e,
            None => {
                // Proposal not yet seen (e.g. history replay order). Log and ack.
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
