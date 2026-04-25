//! Notification abstraction for approval lifecycle events.

#[cfg(feature = "slack")]
use reqwest::Client;

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

// ── SlackWebhookNotifier ──────────────────────────────────────────────────────

/// Posts Slack Block Kit messages to an incoming-webhook URL on every lifecycle event.
///
/// Enable with the `slack` Cargo feature.
#[cfg(feature = "slack")]
pub struct SlackWebhookNotifier {
    client:      Client,
    webhook_url: String,
}

#[cfg(feature = "slack")]
impl SlackWebhookNotifier {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self { client: Client::new(), webhook_url: webhook_url.into() }
    }

    /// Build from `VAULT_SLACK_WEBHOOK_URL` env var.
    pub fn from_env() -> Result<Self, String> {
        let url = std::env::var("VAULT_SLACK_WEBHOOK_URL")
            .map_err(|_| "missing env var: VAULT_SLACK_WEBHOOK_URL".to_string())?;
        Ok(Self::new(url))
    }

    async fn post(&self, text: &str) {
        let result = self
            .client
            .post(&self.webhook_url)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await;
        match result {
            Ok(r) if r.status().is_success() => {}
            Ok(r)  => tracing::warn!(status = %r.status(), "Slack webhook returned non-2xx"),
            Err(e) => tracing::warn!(error = %e, "Slack webhook request failed"),
        }
    }
}

#[cfg(feature = "slack")]
#[async_trait::async_trait]
impl Notifier for SlackWebhookNotifier {
    async fn notify_pending(&self, proposal: &Proposal) {
        self.post(&format!(
            ":hourglass: *Approval pending*\nProposal `{}` for `{}` (service: `{}`)\n> {}",
            proposal.id, proposal.credential_key, proposal.service, proposal.message,
        )).await;
    }

    async fn notify_approved(&self, proposal: &Proposal, approved_by: &str) {
        self.post(&format!(
            ":white_check_mark: *Approved* by `{approved_by}`\nProposal `{}` for `{}`",
            proposal.id, proposal.credential_key,
        )).await;
    }

    async fn notify_rejected(&self, proposal: &Proposal, rejected_by: &str, reason: &str) {
        self.post(&format!(
            ":x: *Rejected* by `{rejected_by}` — {reason}\nProposal `{}` for `{}`",
            proposal.id, proposal.credential_key,
        )).await;
    }
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
