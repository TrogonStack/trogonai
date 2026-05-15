use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::mpsc;

/// Abstraction over a NATS client. Allows injecting a mock in tests.
///
/// `subscribe_bytes` returns an `mpsc::Receiver<Bytes>` so callers can use
/// `tokio::select!` without worrying about pinning opaque stream types.
pub trait NatsClient: Send + Sync + 'static {
    fn request_bytes(
        &self,
        subject: String,
        payload: Bytes,
    ) -> impl std::future::Future<Output = anyhow::Result<Bytes>> + Send + '_;

    fn publish_bytes(
        &self,
        subject: String,
        payload: Bytes,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn publish_with_reply_bytes(
        &self,
        subject: String,
        reply: String,
        payload: Bytes,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_;

    fn subscribe_bytes(
        &self,
        subject: String,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<Bytes>>> + Send + '_;

    fn new_inbox(&self) -> String;
}

// ── Real implementation ───────────────────────────────────────────────────────

impl NatsClient for async_nats::Client {
    async fn request_bytes(&self, subject: String, payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(self.request(subject, payload).await?.payload)
    }

    async fn publish_bytes(&self, subject: String, payload: Bytes) -> anyhow::Result<()> {
        Ok(self.publish(subject, payload).await?)
    }

    async fn publish_with_reply_bytes(
        &self,
        subject: String,
        reply: String,
        payload: Bytes,
    ) -> anyhow::Result<()> {
        Ok(self.publish_with_reply(subject, reply, payload).await?)
    }

    async fn subscribe_bytes(&self, subject: String) -> anyhow::Result<mpsc::Receiver<Bytes>> {
        let mut sub = self.subscribe(subject).await?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                if tx.send(msg.payload).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    fn new_inbox(&self) -> String {
        async_nats::Client::new_inbox(self)
    }
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A mock NATS client for unit tests.
    ///
    /// Internals are wrapped in `Arc` so the struct is `Clone` — clones share the
    /// same queues, matching the behaviour of the real `async_nats::Client`.
    ///
    /// - `queue_request_ok` / `queue_request_err` — pre-load responses for `request_bytes`.
    /// - `add_subscription` — pre-load an `mpsc::Receiver<Bytes>` for `subscribe_bytes`.
    /// - `published()` — returns all messages published so far.
    #[derive(Clone)]
    pub struct MockNatsClient {
        request_responses: Arc<Mutex<VecDeque<Result<Bytes, String>>>>,
        subscriptions: Arc<Mutex<VecDeque<mpsc::Receiver<Bytes>>>>,
        published: Arc<Mutex<Vec<(String, Bytes)>>>,
        inbox_seq: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl MockNatsClient {
        pub fn new() -> Self {
            Self {
                request_responses: Arc::new(Mutex::new(VecDeque::new())),
                subscriptions: Arc::new(Mutex::new(VecDeque::new())),
                published: Arc::new(Mutex::new(Vec::new())),
                inbox_seq: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        pub fn queue_request_ok(&self, response: Bytes) {
            self.request_responses.lock().unwrap().push_back(Ok(response));
        }

        pub fn queue_request_err(&self, msg: &str) {
            self.request_responses.lock().unwrap().push_back(Err(msg.to_string()));
        }

        pub fn add_subscription(&self, rx: mpsc::Receiver<Bytes>) {
            self.subscriptions.lock().unwrap().push_back(rx);
        }

        pub fn published(&self) -> Vec<(String, Bytes)> {
            self.published.lock().unwrap().clone()
        }
    }

    impl NatsClient for MockNatsClient {
        async fn request_bytes(&self, _subject: String, _payload: Bytes) -> anyhow::Result<Bytes> {
            self.request_responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("MockNatsClient: no request response queued"))
                .and_then(|r| r.map_err(|e| anyhow::anyhow!("{e}")))
        }

        async fn publish_bytes(&self, subject: String, payload: Bytes) -> anyhow::Result<()> {
            self.published.lock().unwrap().push((subject, payload));
            Ok(())
        }

        async fn publish_with_reply_bytes(
            &self,
            subject: String,
            _reply: String,
            payload: Bytes,
        ) -> anyhow::Result<()> {
            self.published.lock().unwrap().push((subject, payload));
            Ok(())
        }

        async fn subscribe_bytes(&self, _subject: String) -> anyhow::Result<mpsc::Receiver<Bytes>> {
            self.subscriptions
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("MockNatsClient: no subscription queued"))
        }

        fn new_inbox(&self) -> String {
            let n = self.inbox_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("_INBOX.mock.{n}")
        }
    }
}
