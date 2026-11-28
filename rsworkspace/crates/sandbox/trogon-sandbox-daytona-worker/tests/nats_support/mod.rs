//! A worker served over a throwaway NATS server, against the fake Daytona.

use std::sync::Arc;
use std::time::Duration;

use async_nats::Client;
use async_nats::jetstream;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_daytona_worker::{DaytonaWorkerError, DaytonaWorkerProvider};
use trogon_sandbox_protocol::{nats_transfer, subjects};

use crate::support::{Boxed, Fake, request_json};

pub const SILENCE: Duration = Duration::from_millis(700);

const CONNECT_ATTEMPTS: u32 = 30;
const CONNECT_ATTEMPT_BOUND: Duration = Duration::from_secs(5);
const CONNECT_RETRY_PAUSE: Duration = Duration::from_millis(500);

/// A throwaway NATS server running in a container.
pub struct Nats {
    server: JetStreamTestServer,
}

impl Nats {
    pub async fn start() -> Self {
        Self {
            server: JetStreamTestServer::start().await,
        }
    }

    /// A connection to the server, retried for a while: a container that just started may accept connections later
    /// than the fixed bound the shared fixture allows, most often when many test binaries start at once.
    pub async fn client(&self) -> Client {
        for _ in 0..CONNECT_ATTEMPTS {
            let connected = async_nats::ConnectOptions::new()
                .connection_timeout(CONNECT_ATTEMPT_BOUND)
                .connect(self.server.address())
                .await;
            if let Ok(client) = connected {
                return client;
            }
            tokio::time::sleep(CONNECT_RETRY_PAUSE).await;
        }
        // The fixture reports a server that never accepts a connection.
        self.server.client().await
    }

    /// A connection that does not check subjects before sending them.
    pub async fn unchecked_client(&self) -> Result<Client, Boxed> {
        Ok(async_nats::ConnectOptions::new()
            .skip_subject_validation(true)
            .connect(self.server.address())
            .await?)
    }
}

/// A worker serving a provider on a NATS server until stopped.
pub struct Running {
    pub nats: Nats,
    pub client: Client,
    pub fake: Fake,
    pub provider: Arc<DaytonaWorkerProvider>,
    pub worker_id: WorkerId,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), DaytonaWorkerError>>>,
}

pub fn dispatch_json(lease: &str) -> Value {
    json!({"lease_id": lease, "request": request_json()})
}

impl Running {
    pub async fn start() -> Result<Self, Boxed> {
        Self::start_with_ledger(None).await
    }

    /// A worker that keeps its targeted dispatches in the ledger at `ledger`, when one is given.
    pub async fn start_with_ledger(ledger: Option<&std::path::Path>) -> Result<Self, Boxed> {
        let nats = Nats::start().await;
        let client = nats.client().await;
        let fake = Fake::with_sandbox().await?;
        let provider = fake.provider()?;
        let worker_id = WorkerId::new("worker-1")?;
        if let Some(path) = ledger {
            provider.configure_dispatch_ledger(Arc::new(trogon_sandbox_protocol::AttemptLedger::open(
                path,
                trogon_sandbox_contract::ProviderKind::Daytona,
                Some(worker_id.clone()),
            )?))?;
        }
        let provider = Arc::new(provider);
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (client, provider, worker_id, cancel) =
                (client.clone(), provider.clone(), worker_id.clone(), cancel.clone());
            async move {
                let js = jetstream::new(client.clone());
                trogon_sandbox_daytona_worker::run(client, js, provider, &worker_id, cancel).await
            }
        });
        let running = Self {
            nats,
            client,
            fake,
            provider,
            worker_id,
            cancel,
            task: Some(task),
        };
        running.wait_until_serving().await?;
        Ok(running)
    }

    /// An answer on the lease subject means the verbs are served too, since the worker subscribes to it last.
    async fn wait_until_serving(&self) -> Result<(), Boxed> {
        for _ in 0..100 {
            let probe = self.client.request("sandbox.provider.daytona.lease", "{}".into());
            if matches!(tokio::time::timeout(Duration::from_secs(1), probe).await, Ok(Ok(_))) {
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("the worker never started serving".into())
    }

    /// Stops the worker and waits for it to finish.
    pub async fn stop(&mut self) -> Result<(), Boxed> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }

    /// Sends one JSON request, through the transfer protocol, and returns the JSON reply.
    pub async fn ask(&self, subject: &str, payload: &Value) -> Result<Value, Boxed> {
        let reply = nats_transfer::request(
            &self.client,
            subject,
            serde_json::to_vec(payload)?.into(),
            None,
            Duration::from_secs(30),
        )
        .await?;
        Ok(serde_json::from_slice(&reply)?)
    }

    /// Dispatches a lease to the worker.
    pub async fn dispatch(&self, lease: &str) -> Result<Value, Boxed> {
        self.ask("sandbox.provider.daytona.lease", &dispatch_json(lease)).await
    }

    /// Sends a lease verb such as `run_command` or `pause`.
    pub async fn verb(&self, verb: &str, lease: &str, mut payload: Value) -> Result<Value, Boxed> {
        payload["lease_id"] = json!(lease);
        self.ask(&format!("sandbox.provider.daytona.{verb}.{lease}"), &payload)
            .await
    }

    pub fn snapshot_export_subject(&self) -> String {
        subjects::worker_snapshot_export(&self.worker_id)
    }

    /// Publishes without a reply subject.
    pub async fn publish(&self, subject: String, payload: &Value) -> Result<(), Boxed> {
        self.client
            .publish(subject, serde_json::to_vec(payload)?.into())
            .await?;
        self.client.flush().await?;
        Ok(())
    }

    /// Asserts that a request on `subject` gets no answer.
    pub async fn silent(&self, subject: &str, payload: &Value) -> Result<(), Boxed> {
        let request = self
            .client
            .request(subject.to_owned(), serde_json::to_vec(payload)?.into());
        let outcome = tokio::time::timeout(SILENCE, request).await;
        assert!(!matches!(outcome, Ok(Ok(_))), "unexpected reply on {subject}");
        Ok(())
    }

    pub async fn cancel_lease(&self, lease: &str) -> Result<(), Boxed> {
        self.publish(format!("sandbox.provider.daytona.cancel.{lease}"), &json!({}))
            .await
    }
}

/// Polls until `check` holds.
pub async fn until(mut check: impl FnMut() -> bool) -> Result<(), Boxed> {
    for _ in 0..200 {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("condition not reached".into())
}
