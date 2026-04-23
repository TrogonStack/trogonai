use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{Client as AcpClient, SessionNotification};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;

/// Publishes ACP `SessionNotification` messages during a prompt run.
///
/// The real implementation delegates to `NatsClientProxy`; the test mock
/// silently discards all notifications.
#[async_trait(?Send)]
pub trait PromptEventClient {
    async fn session_notification(
        &self,
        notif: SessionNotification,
    ) -> agent_client_protocol::Result<()>;
}

/// Abstraction over the raw NATS transport used by `TrogonAgent`.
///
/// Covers three distinct operations:
/// 1. Fire-and-forget publish (session-ready, cancel signals).
/// 2. Cancel-subscription (returns a oneshot that fires once cancelled).
/// 3. Creating the per-prompt ACP notification client.
#[async_trait(?Send)]
pub trait SessionNotifier: Clone {
    /// Publish `payload` to `subject` (fire-and-forget; errors are swallowed).
    async fn publish(&self, subject: String, payload: Bytes);

    /// Schedule a fire-and-forget publish after `delay`.
    ///
    /// The real implementation uses `tokio::spawn` (NATS client is `Send`).
    /// Test mocks record the publish synchronously and ignore the delay.
    fn schedule_publish(&self, subject: String, payload: Bytes, delay: Duration);

    /// Subscribe to `subject` for a cancel signal.
    ///
    /// Returns a `oneshot::Receiver` that resolves the first time a message
    /// arrives on that subject.  Returns `None` if the subscribe call fails.
    async fn subscribe_cancel(
        &self,
        subject: String,
    ) -> Option<tokio::sync::oneshot::Receiver<()>>;

    /// Subscribe to `subject` for mid-turn steer messages.
    ///
    /// Each message payload is decoded as UTF-8 and forwarded to the returned
    /// `mpsc::Receiver`.  The channel stays open for the lifetime of the
    /// prompt; the spawned forwarder task exits when the receiver is dropped.
    /// Returns `None` if the subscribe call fails.
    async fn subscribe_steer(
        &self,
        subject: String,
    ) -> Option<tokio::sync::mpsc::Receiver<String>>;

    /// Build a notification client bound to the given ACP session.
    fn make_prompt_client(
        &self,
        session_id: AcpSessionId,
        prefix: AcpPrefix,
    ) -> Box<dyn PromptEventClient>;
}

// ── Real NATS implementation ──────────────────────────────────────────────────

/// `SessionNotifier` backed by a live `async_nats::Client`.
#[derive(Clone)]
pub struct NatsSessionNotifier {
    client: async_nats::Client,
}

impl NatsSessionNotifier {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[async_trait(?Send)]
impl SessionNotifier for NatsSessionNotifier {
    async fn publish(&self, subject: String, payload: Bytes) {
        let _ = self.client.publish(subject, payload).await;
    }

    fn schedule_publish(&self, subject: String, payload: Bytes, delay: Duration) {
        let client = self.client.clone();
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let _ = client.publish(subject, payload).await;
        });
    }

    async fn subscribe_cancel(
        &self,
        subject: String,
    ) -> Option<tokio::sync::oneshot::Receiver<()>> {
        let mut sub = match self.client.subscribe(subject).await {
            Ok(s) => s,
            Err(_) => return None,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_local(async move {
            if sub.next().await.is_some() {
                let _ = tx.send(());
            }
        });
        Some(rx)
    }

    async fn subscribe_steer(
        &self,
        subject: String,
    ) -> Option<tokio::sync::mpsc::Receiver<String>> {
        let mut sub = match self.client.subscribe(subject).await {
            Ok(s) => s,
            Err(_) => return None,
        };
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::task::spawn_local(async move {
            while let Some(msg) = sub.next().await {
                let text = String::from_utf8_lossy(&msg.payload).to_string();
                if tx.send(text).await.is_err() {
                    break;
                }
            }
        });
        Some(rx)
    }

    fn make_prompt_client(
        &self,
        session_id: AcpSessionId,
        prefix: AcpPrefix,
    ) -> Box<dyn PromptEventClient> {
        Box::new(NatsPromptEventClient {
            proxy: NatsClientProxy::new(
                self.client.clone(),
                session_id,
                prefix,
                Duration::from_secs(30),
            ),
        })
    }
}

struct NatsPromptEventClient {
    proxy: NatsClientProxy<async_nats::Client>,
}

#[async_trait(?Send)]
impl PromptEventClient for NatsPromptEventClient {
    async fn session_notification(
        &self,
        notif: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        self.proxy.session_notification(notif).await
    }
}

// ── Mock (test-helpers feature) ───────────────────────────────────────────────

#[cfg(feature = "test-helpers")]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    /// In-memory `SessionNotifier` for unit tests.
    ///
    /// Captures all published messages and exposes a `trigger_cancel` helper
    /// that fires the cancel signal without actual NATS.
    #[derive(Clone, Default)]
    pub struct MockSessionNotifier {
        published: Arc<Mutex<Vec<(String, Bytes)>>>,
        cancel_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    }

    impl MockSessionNotifier {
        pub fn new() -> Self {
            Self::default()
        }

        /// Send the cancel signal as if a remote NATS publish had arrived.
        pub fn trigger_cancel(&self) {
            if let Some(tx) = self.cancel_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }

        /// Return all (subject, payload) pairs published via `publish()`.
        pub fn published(&self) -> Vec<(String, Bytes)> {
            self.published.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait(?Send)]
    impl SessionNotifier for MockSessionNotifier {
        async fn publish(&self, subject: String, payload: Bytes) {
            self.published.lock().unwrap().push((subject, payload));
        }

        fn schedule_publish(&self, subject: String, payload: Bytes, _delay: Duration) {
            self.published.lock().unwrap().push((subject, payload));
        }

        async fn subscribe_cancel(&self, _subject: String) -> Option<oneshot::Receiver<()>> {
            let (tx, rx) = oneshot::channel();
            *self.cancel_tx.lock().unwrap() = Some(tx);
            Some(rx)
        }

        async fn subscribe_steer(
            &self,
            _subject: String,
        ) -> Option<tokio::sync::mpsc::Receiver<String>> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Some(rx)
        }

        fn make_prompt_client(
            &self,
            _session_id: AcpSessionId,
            _prefix: AcpPrefix,
        ) -> Box<dyn PromptEventClient> {
            Box::new(NullPromptEventClient)
        }
    }

    /// `PromptEventClient` that silently discards every notification.
    pub struct NullPromptEventClient;

    #[async_trait::async_trait(?Send)]
    impl PromptEventClient for NullPromptEventClient {
        async fn session_notification(
            &self,
            _notif: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            Ok(())
        }
    }
}
