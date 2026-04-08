use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::Client as _;
use agent_client_protocol::SessionNotification;
use async_trait::async_trait;
use tracing::warn;

/// Abstraction over the ACP session notification channel.
///
/// Decouples the agent core from NATS so the same logic can be tested with
/// an in-memory mock and, eventually, run inside a WASM sandbox with a
/// different transport.
#[async_trait(?Send)]
pub trait SessionNotifier {
    async fn notify(&self, notification: SessionNotification);
}

/// Blanket impl so `Arc<T>` can be used wherever `T: SessionNotifier`.
#[async_trait(?Send)]
impl<T: SessionNotifier> SessionNotifier for Arc<T> {
    async fn notify(&self, notification: SessionNotification) {
        (**self).notify(notification).await;
    }
}

// ── Production implementation ─────────────────────────────────────────────────

/// Routes ACP `SessionNotification`s to clients over NATS.
pub struct NatsSessionNotifier {
    nats: async_nats::Client,
    acp_prefix: AcpPrefix,
}

impl NatsSessionNotifier {
    pub fn new(nats: async_nats::Client, acp_prefix: AcpPrefix) -> Self {
        Self { nats, acp_prefix }
    }
}

#[async_trait(?Send)]
impl SessionNotifier for NatsSessionNotifier {
    async fn notify(&self, notification: SessionNotification) {
        let session_id = match AcpSessionId::try_from(&notification.session_id) {
            Ok(id) => id,
            Err(e) => {
                warn!(error = %e, "xai: invalid session id in notification — skipping");
                return;
            }
        };
        let client = NatsClientProxy::new(
            self.nats.clone(),
            session_id,
            self.acp_prefix.clone(),
            Duration::from_secs(30),
        );
        if let Err(e) = client.session_notification(notification).await {
            warn!(error = %e, "xai: failed to send notification");
        }
    }
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
pub struct MockSessionNotifier {
    pub notifications: std::sync::Mutex<Vec<SessionNotification>>,
}

#[cfg(any(test, feature = "test-helpers"))]
impl MockSessionNotifier {
    pub fn new() -> Self {
        Self { notifications: std::sync::Mutex::new(Vec::new()) }
    }
}

#[cfg(any(test, feature = "test-helpers"))]
impl Default for MockSessionNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-helpers"))]
#[async_trait(?Send)]
impl SessionNotifier for MockSessionNotifier {
    async fn notify(&self, notification: SessionNotification) {
        self.notifications.lock().unwrap().push(notification);
    }
}
