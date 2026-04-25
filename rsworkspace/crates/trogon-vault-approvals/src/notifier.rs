//! Notification abstraction for approval lifecycle events.

use crate::proposal::Proposal;

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait Notifier: Send + Sync + 'static {
    /// Called when a new proposal is created (status: Pending).
    async fn notify_pending(&self, proposal: &Proposal);

    /// Called after a proposal is approved and the credential is stored.
    async fn notify_approved(&self, proposal: &Proposal, approved_by: &str);

    /// Called after a proposal is rejected.
    async fn notify_rejected(&self, proposal: &Proposal, rejected_by: &str, reason: &str);
}

// ── NoopNotifier ──────────────────────────────────────────────────────────────

/// Does nothing — suitable for tests and deployments without notifications.
pub struct NoopNotifier;

#[async_trait::async_trait]
impl Notifier for NoopNotifier {
    async fn notify_pending(&self, _proposal: &Proposal) {}
    async fn notify_approved(&self, _proposal: &Proposal, _approved_by: &str) {}
    async fn notify_rejected(&self, _proposal: &Proposal, _rejected_by: &str, _reason: &str) {}
}

// ── LoggingNotifier ───────────────────────────────────────────────────────────

/// Emits tracing events for every approval lifecycle transition.
pub struct LoggingNotifier;

#[async_trait::async_trait]
impl Notifier for LoggingNotifier {
    async fn notify_pending(&self, proposal: &Proposal) {
        tracing::info!(
            id             = %proposal.id,
            credential_key = %proposal.credential_key,
            service        = %proposal.service,
            message        = %proposal.message,
            "approval pending"
        );
    }

    async fn notify_approved(&self, proposal: &Proposal, approved_by: &str) {
        tracing::info!(
            id          = %proposal.id,
            approved_by = %approved_by,
            "proposal approved"
        );
    }

    async fn notify_rejected(&self, proposal: &Proposal, rejected_by: &str, reason: &str) {
        tracing::info!(
            id          = %proposal.id,
            rejected_by = %rejected_by,
            reason      = %reason,
            "proposal rejected"
        );
    }
}
