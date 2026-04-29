//! Notification abstraction for approval lifecycle events.

#[cfg(feature = "slack")]
use reqwest::Client;

use crate::proposal::Proposal;

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait Notifier: Send + Sync + 'static {
    /// Called when a new proposal is created (status: Pending).
    ///
    /// `vault_name` is the approval vault name (e.g. `"prod"`). Implementations
    /// should include it in the notification so the operator knows which NATS
    /// subjects to use when approving or rejecting.
    async fn notify_pending(&self, proposal: &Proposal, vault_name: &str);

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
    async fn notify_pending(&self, _proposal: &Proposal, _vault_name: &str) {}
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
    async fn notify_pending(&self, proposal: &Proposal, vault_name: &str) {
        self.post(&format!(
            ":hourglass: *Approval pending*\n\
             Proposal `{id}` requests `{key}` for `{svc}`\n\
             > {msg}\n\n\
             *To approve*, publish to `vault.proposals.{vault}.approve`:\n\
             ```{{\"proposal_id\":\"{id}\",\"approved_by\":\"<name>\",\"plaintext\":\"<api-key>\"}}```\n\
             *To reject*, publish to `vault.proposals.{vault}.reject`:\n\
             ```{{\"proposal_id\":\"{id}\",\"rejected_by\":\"<name>\",\"reason\":\"<optional>\"}}```",
            id    = proposal.id,
            key   = proposal.credential_key,
            svc   = proposal.service,
            msg   = proposal.message,
            vault = vault_name,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::{Proposal, ProposalStatus};

    fn pending() -> Proposal {
        Proposal {
            id:             "prop_abc".into(),
            credential_key: "tok_stripe_prod_abc".into(),
            service:        "api.stripe.com".into(),
            message:        "need access for payments".into(),
            requested_at:   None,
            status:         ProposalStatus::Pending,
        }
    }

    fn approved() -> Proposal {
        Proposal {
            status: ProposalStatus::Approved { approved_by: "mario".into() },
            ..pending()
        }
    }

    fn rejected() -> Proposal {
        Proposal {
            status: ProposalStatus::Rejected { rejected_by: "luigi".into(), reason: "not authorised".into() },
            ..pending()
        }
    }

    #[tokio::test]
    async fn noop_notifier_does_not_panic() {
        let n = NoopNotifier;
        n.notify_pending(&pending(), "prod").await;
        n.notify_approved(&approved(), "mario").await;
        n.notify_rejected(&rejected(), "luigi", "not authorised").await;
    }

    #[tokio::test]
    async fn logging_notifier_does_not_panic() {
        let n = LoggingNotifier;
        n.notify_pending(&pending(), "staging").await;
        n.notify_approved(&approved(), "mario").await;
        n.notify_rejected(&rejected(), "luigi", "expired").await;
    }

    #[cfg(feature = "slack")]
    #[test]
    fn slack_from_env_errors_when_var_missing() {
        unsafe { std::env::remove_var("VAULT_SLACK_WEBHOOK_URL"); }
        assert!(SlackWebhookNotifier::from_env().is_err());
    }

    #[cfg(feature = "slack")]
    #[test]
    fn slack_from_env_succeeds_when_var_set() {
        unsafe { std::env::set_var("VAULT_SLACK_WEBHOOK_URL", "https://hooks.slack.example/T123/B456/xyz"); }
        let result = SlackWebhookNotifier::from_env();
        unsafe { std::env::remove_var("VAULT_SLACK_WEBHOOK_URL"); }
        assert!(result.is_ok());
    }

    // ── SlackWebhookNotifier HTTP tests ───────────────────────────────────────

    #[cfg(feature = "slack")]
    mod slack_http_tests {
        use super::*;
        use httpmock::MockServer;
        use httpmock::Method::POST;

        fn proposal() -> Proposal {
            pending()
        }

        #[tokio::test]
        async fn notify_pending_posts_to_webhook() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/webhook");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_pending(&proposal(), "prod").await;
            mock.assert();
        }

        #[tokio::test]
        async fn notify_pending_body_contains_proposal_fields() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST)
                    .path("/webhook")
                    .body_contains("prop_abc")
                    .body_contains("tok_stripe_prod_abc")
                    .body_contains("prod");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_pending(&proposal(), "prod").await;
            mock.assert();
        }

        #[tokio::test]
        async fn notify_approved_posts_to_webhook() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/webhook");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_approved(&approved(), "mario").await;
            mock.assert();
        }

        #[tokio::test]
        async fn notify_approved_body_contains_approver() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST)
                    .path("/webhook")
                    .body_contains("mario")
                    .body_contains("prop_abc");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_approved(&approved(), "mario").await;
            mock.assert();
        }

        #[tokio::test]
        async fn notify_rejected_posts_to_webhook() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/webhook");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_rejected(&rejected(), "luigi", "not authorised").await;
            mock.assert();
        }

        #[tokio::test]
        async fn notify_rejected_body_contains_rejector_and_reason() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST)
                    .path("/webhook")
                    .body_contains("luigi")
                    .body_contains("not authorised");
                then.status(200);
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_rejected(&rejected(), "luigi", "not authorised").await;
            mock.assert();
        }

        #[tokio::test]
        async fn non_2xx_response_does_not_panic() {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(POST).path("/webhook");
                then.status(500).body("internal error");
            });

            let notifier = SlackWebhookNotifier::new(format!("{}/webhook", server.base_url()));
            notifier.notify_pending(&proposal(), "prod").await;
            // Must complete without panic despite non-2xx
        }
    }
}

#[async_trait::async_trait]
impl Notifier for LoggingNotifier {
    async fn notify_pending(&self, proposal: &Proposal, vault_name: &str) {
        tracing::info!(
            id             = %proposal.id,
            credential_key = %proposal.credential_key,
            service        = %proposal.service,
            message        = %proposal.message,
            vault          = %vault_name,
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
