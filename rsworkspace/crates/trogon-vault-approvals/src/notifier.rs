//! Notification abstraction for approval lifecycle events.

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

/// Sends a JSON POST to a Slack incoming-webhook URL.
///
/// Implemented by `reqwest::Client` for production; unit tests use
/// [`MockSlackHttpClient`] defined in the test module.
#[cfg(feature = "slack")]
pub(crate) trait SlackHttpClient: Send + Sync + 'static {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        body: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = Result<u16, String>> + Send + 'a;
}

#[cfg(feature = "slack")]
impl SlackHttpClient for reqwest::Client {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        body: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = Result<u16, String>> + Send + 'a {
        async move {
            let resp = self
                .post(url)
                .json(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            Ok(resp.status().as_u16())
        }
    }
}

/// Posts Slack Block Kit messages to an incoming-webhook URL on every lifecycle event.
///
/// Enable with the `slack` Cargo feature.
#[cfg(feature = "slack")]
pub struct SlackWebhookNotifier<H = reqwest::Client> {
    client:      H,
    webhook_url: String,
}

#[cfg(feature = "slack")]
impl SlackWebhookNotifier<reqwest::Client> {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self { client: reqwest::Client::new(), webhook_url: webhook_url.into() }
    }

    /// Build from `VAULT_SLACK_WEBHOOK_URL` env var.
    pub fn from_env() -> Result<Self, String> {
        let url = std::env::var("VAULT_SLACK_WEBHOOK_URL")
            .map_err(|_| "missing env var: VAULT_SLACK_WEBHOOK_URL".to_string())?;
        Ok(Self::new(url))
    }
}

#[cfg(feature = "slack")]
impl<H: SlackHttpClient> SlackWebhookNotifier<H> {
    pub fn with_client(webhook_url: impl Into<String>, client: H) -> Self {
        Self { client, webhook_url: webhook_url.into() }
    }

    async fn post(&self, text: &str) {
        let body = serde_json::json!({ "text": text });
        match self.client.post_json(&self.webhook_url, &body).await {
            Ok(status) if (200..300).contains(&status) => {}
            Ok(status) => tracing::warn!(status, "Slack webhook returned non-2xx"),
            Err(e) => tracing::warn!(error = %e, "Slack webhook request failed"),
        }
    }
}

#[cfg(feature = "slack")]
#[async_trait::async_trait]
impl<H: SlackHttpClient> Notifier for SlackWebhookNotifier<H> {
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

    // ── SlackWebhookNotifier unit tests via MockSlackHttpClient ───────────────

    #[cfg(feature = "slack")]
    mod slack_tests {
        use std::sync::Mutex;
        use super::*;

        #[derive(Default)]
        struct MockSlackHttpClient {
            response_status: u16,
            last_url:  Mutex<Option<String>>,
            last_body: Mutex<Option<serde_json::Value>>,
        }

        impl MockSlackHttpClient {
            fn ok() -> Self {
                Self { response_status: 200, ..Default::default() }
            }

            fn error(status: u16) -> Self {
                Self { response_status: status, ..Default::default() }
            }

            fn last_url(&self) -> Option<String> {
                self.last_url.lock().unwrap().clone()
            }

            fn last_body(&self) -> Option<serde_json::Value> {
                self.last_body.lock().unwrap().clone()
            }
        }

        impl SlackHttpClient for MockSlackHttpClient {
            fn post_json<'a>(
                &'a self,
                url: &'a str,
                body: &'a serde_json::Value,
            ) -> impl std::future::Future<Output = Result<u16, String>> + Send + 'a {
                *self.last_url.lock().unwrap() = Some(url.to_string());
                *self.last_body.lock().unwrap() = Some(body.clone());
                let status = self.response_status;
                async move { Ok(status) }
            }
        }

        fn notifier(mock: MockSlackHttpClient) -> SlackWebhookNotifier<MockSlackHttpClient> {
            SlackWebhookNotifier::with_client("https://hooks.slack.example/webhook", mock)
        }

        #[tokio::test]
        async fn notify_pending_posts_to_webhook() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_pending(&pending(), "prod").await;
            assert!(n.client.last_url().is_some(), "expected a POST to be made");
        }

        #[tokio::test]
        async fn notify_pending_body_contains_proposal_fields() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_pending(&pending(), "prod").await;
            let body = n.client.last_body().unwrap().to_string();
            assert!(body.contains("prop_abc"), "missing proposal id");
            assert!(body.contains("tok_stripe_prod_abc"), "missing credential key");
            assert!(body.contains("prod"), "missing vault name");
        }

        #[tokio::test]
        async fn notify_approved_posts_to_webhook() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_approved(&approved(), "mario").await;
            assert!(n.client.last_url().is_some());
        }

        #[tokio::test]
        async fn notify_approved_body_contains_approver() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_approved(&approved(), "mario").await;
            let body = n.client.last_body().unwrap().to_string();
            assert!(body.contains("mario"), "missing approver name");
            assert!(body.contains("prop_abc"), "missing proposal id");
        }

        #[tokio::test]
        async fn notify_rejected_posts_to_webhook() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_rejected(&rejected(), "luigi", "not authorised").await;
            assert!(n.client.last_url().is_some());
        }

        #[tokio::test]
        async fn notify_rejected_body_contains_rejector_and_reason() {
            let mock = MockSlackHttpClient::ok();
            let n = notifier(mock);
            n.notify_rejected(&rejected(), "luigi", "not authorised").await;
            let body = n.client.last_body().unwrap().to_string();
            assert!(body.contains("luigi"), "missing rejector name");
            assert!(body.contains("not authorised"), "missing reason");
        }

        #[tokio::test]
        async fn non_2xx_response_does_not_panic() {
            let mock = MockSlackHttpClient::error(500);
            let n = notifier(mock);
            n.notify_pending(&pending(), "prod").await;
            // Must complete without panic despite non-2xx
        }
    }
}
