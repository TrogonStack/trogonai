// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::Arc;
use std::time::Duration;

use async_nats::client::RequestErrorKind;
use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseId, WorkerId};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bWorkerError, E2bWorkerProvider, Envd, run};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, NatsTransferError, nats_transfer};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;

const SILENCE: Duration = Duration::from_millis(700);
const PATIENCE: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(100);

/// A worker running in this process against a private NATS server, holding one lease.
pub struct Running {
    pub nats: JetStreamTestServer,
    pub client: Client,
    pub provider: Arc<E2bWorkerProvider>,
    pub lease: LeaseId,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), E2bWorkerError>>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Running {
    /// Starts a worker over `provider`, which already holds `lease`.
    pub async fn start(
        nats: JetStreamTestServer,
        provider: Arc<E2bWorkerProvider>,
        lease: LeaseId,
        worker_id: WorkerId,
    ) -> Result<Self, Boxed> {
        let client = nats.client().await;
        let worker_client = nats.client().await;
        let envd = Envd::new(Duration::from_secs(3))?;
        let terminals = Arc::new(E2bTerminals::new(
            Arc::new(E2bGuestShell::new(provider.clone(), envd)),
            worker_client.clone(),
        ));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (provider, cancel) = (provider.clone(), cancel.clone());
            async move {
                run(
                    worker_client.clone(),
                    async_nats::jetstream::new(worker_client),
                    provider,
                    &worker_id,
                    cancel,
                    terminals,
                    AuthorityConfig::new(AuthorityMode::Off),
                )
                .await
            }
        });
        let running = Self {
            nats,
            client,
            provider,
            lease,
            cancel,
            task: Some(task),
        };
        running.wait_until_serving().await?;
        Ok(running)
    }

    /// Waits until the worker has subscribed everything it serves.
    async fn wait_until_serving(&self) -> Result<(), Boxed> {
        tokio::time::timeout(Duration::from_secs(15), self.provider.wait_ready()).await?;
        Ok(())
    }

    /// Requests `subject` and returns the reply, or `None` when the worker answers nothing in time.
    pub async fn ask(&self, subject: &str, body: &[u8]) -> Result<Option<Value>, Boxed> {
        self.request(subject, body, PATIENCE).await
    }

    /// Requests `subject` from a worker that is expected to stay silent, so a short wait settles it.
    pub async fn ask_silent(&self, subject: &str, body: &[u8]) -> Result<Option<Value>, Boxed> {
        self.request(subject, body, SILENCE).await
    }

    /// Requests `subject`, retrying while the server still sees no subscriber, and waits `wait` for each answer.
    async fn request(&self, subject: &str, body: &[u8], wait: Duration) -> Result<Option<Value>, Boxed> {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            match nats_transfer::request(&self.client, subject, Bytes::copy_from_slice(body), None, wait).await {
                Ok(reply) => return Ok(Some(serde_json::from_slice(&reply)?)),
                Err(NatsTransferError::Request(error))
                    if error.kind() == RequestErrorKind::NoResponders && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(RETRY).await;
                }
                Err(_) => return Ok(None),
            }
        }
    }

    /// Requests `subject` with a body that declares a transfer version the worker does not know.
    pub async fn ask_with_unknown_transfer(&self, subject: &str, body: &[u8]) -> Result<Option<Value>, Boxed> {
        self.request(subject, b"{}", SILENCE).await?;
        let mut headers = HeaderMap::new();
        headers.insert("Sandbox-Transfer", "no-such-version");
        let inbox = self.client.new_inbox();
        let mut replies = self.client.subscribe(inbox.clone()).await?;
        self.client
            .publish_with_reply_and_headers(subject.to_owned(), inbox, headers, Bytes::copy_from_slice(body))
            .await?;
        self.client.flush().await?;
        match tokio::time::timeout(SILENCE, replies.next()).await {
            Ok(Some(message)) => Ok(Some(serde_json::from_slice(&message.payload)?)),
            _ => Ok(None),
        }
    }

    /// Publishes `body` on `subject` with no reply subject.
    pub async fn tell(&self, subject: &str, body: &[u8]) -> Result<(), Boxed> {
        self.request(subject, b"{}", SILENCE).await?;
        self.client
            .publish(subject.to_owned(), Bytes::copy_from_slice(body))
            .await?;
        self.client.flush().await?;
        tokio::time::sleep(SILENCE).await;
        Ok(())
    }
}

/// The subject of the lease verb `verb` for `lease`.
pub fn verb(verb: &str, lease: &LeaseId) -> String {
    format!("sandbox.provider.e2b.{verb}.{lease}")
}
