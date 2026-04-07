use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{Client as _, SessionId, SessionNotification};
use async_trait::async_trait;
use tracing::warn;

/// Abstraction over the ACP session notification channel so agent tests can
/// use an in-process mock without a NATS connection.
#[async_trait(?Send)]
pub trait SessionNotifier {
    /// Send a session notification. Implementations are fire-and-forget: errors
    /// are logged and swallowed rather than propagated to the caller.
    async fn session_notification(&self, notif: SessionNotification);
}

// ── NATS implementation ───────────────────────────────────────────────────────

/// Forwards session notifications to a real NATS server via `NatsClientProxy`.
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
    async fn session_notification(&self, notif: SessionNotification) {
        let session_id = SessionId::from(notif.session_id.to_string());
        let acp_session_id = match AcpSessionId::try_from(&session_id) {
            Ok(id) => id,
            Err(e) => {
                warn!(error = %e, "xai: invalid session ID — cannot send notification");
                return;
            }
        };
        let client = NatsClientProxy::new(
            self.nats.clone(),
            acp_session_id,
            self.acp_prefix.clone(),
            Duration::from_secs(30),
        );
        if let Err(e) = client.session_notification(notif).await {
            warn!(error = %e, "xai: failed to send session notification");
        }
    }
}

// ── Mock implementation (test-helpers only) ───────────────────────────────────

#[cfg(feature = "test-helpers")]
pub struct MockSessionNotifier {
    pub notifications: std::sync::Mutex<Vec<SessionNotification>>,
}

#[cfg(feature = "test-helpers")]
impl MockSessionNotifier {
    pub fn new() -> Self {
        Self { notifications: std::sync::Mutex::new(Vec::new()) }
    }
}

#[cfg(feature = "test-helpers")]
impl Default for MockSessionNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "test-helpers")]
#[async_trait(?Send)]
impl SessionNotifier for MockSessionNotifier {
    async fn session_notification(&self, notif: SessionNotification) {
        self.notifications.lock().unwrap().push(notif);
    }
}
