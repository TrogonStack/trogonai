// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A worker served over a throwaway NATS server, against a loopback fake of the Vercel API.

use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::Client;
use async_nats::jetstream;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_protocol::{nats_transfer, subjects};
use trogon_sandbox_vercel_worker::{VercelPlan, VercelWorkerError, VercelWorkerProvider};

use crate::support::{Boxed, Reply, Request, Server};

pub const SILENCE: Duration = Duration::from_millis(700);

/// What the fake Vercel was asked and how it should answer.
pub struct State {
    calls: Mutex<Vec<String>>,
    creates: AtomicUsize,
    create_delay_ms: AtomicU64,
    create_status: AtomicU16,
    delete_status: AtomicU16,
    stop_status: AtomicU16,
    get_status: AtomicU16,
    read_content: Mutex<Vec<u8>>,
    write_bytes: AtomicUsize,
}

pub struct Fake {
    pub server: Server,
    pub state: Arc<State>,
}

fn sbx_json(name: &str) -> Value {
    json!({
        "sandbox": {"name": name, "persistent": true},
        "session": {"id": "sess-1", "status": "running", "region": "iad1"},
        "routes": [{"url": "https://sbx.example.test", "port": 8080}],
    })
}

fn answer(state: &State, request: &Request) -> Reply {
    state
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(format!("{} {}", request.method, request.route()));
    let route = request.route();
    match request.method.as_str() {
        "POST" if route.ends_with("/v2/sandboxes") || route.ends_with("/v3/sandboxes") => {
            std::thread::sleep(Duration::from_millis(state.create_delay_ms.load(Ordering::SeqCst)));
            let number = state.creates.fetch_add(1, Ordering::SeqCst) + 1;
            Reply::json(
                state.create_status.load(Ordering::SeqCst),
                sbx_json(&format!("sbx-{number}")),
            )
        }
        "DELETE" => Reply::json(state.delete_status.load(Ordering::SeqCst), json!({})),
        "GET" => Reply::json(state.get_status.load(Ordering::SeqCst), sbx_json("sbx-1")),
        "POST" if route.ends_with("/stop") => Reply::json(state.stop_status.load(Ordering::SeqCst), json!({})),
        "POST" if route.ends_with("/snapshot") => {
            Reply::json(200, json!({"snapshot": {"id": "snap-1", "status": "created"}}))
        }
        "POST" if route.ends_with("/fs/write") => {
            state.write_bytes.store(request.body.len(), Ordering::SeqCst);
            Reply::json(200, json!({}))
        }
        "POST" if route.ends_with("/fs/read") => Reply::Complete(
            200,
            state
                .read_content
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        ),
        _ => Reply::json(404, json!({"error": {"message": "not found"}})),
    }
}

impl Fake {
    pub async fn start() -> Result<Self, Boxed> {
        let state = Arc::new(State {
            calls: Mutex::new(Vec::new()),
            creates: AtomicUsize::new(0),
            create_delay_ms: AtomicU64::new(0),
            create_status: AtomicU16::new(200),
            delete_status: AtomicU16::new(200),
            stop_status: AtomicU16::new(200),
            get_status: AtomicU16::new(200),
            read_content: Mutex::new(b"hello".to_vec()),
            write_bytes: AtomicUsize::new(0),
        });
        let seen = Arc::clone(&state);
        let server = Server::start(move |request| answer(&seen, &request)).await?;
        Ok(Self { server, state })
    }

    /// How many requests of `method` whose route ends with `suffix` the fake has seen.
    pub fn count(&self, method: &str, suffix: &str) -> usize {
        let prefix = format!("{method} ");
        let calls = self
            .state
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls
            .iter()
            .filter(|call| call.starts_with(&prefix) && call.ends_with(suffix))
            .count()
    }

    pub fn creates(&self) -> usize {
        self.count("POST", "/sandboxes")
    }

    pub fn deletes(&self) -> usize {
        self.count("DELETE", "/sbx-1")
    }

    pub fn create_status(&self, status: u16) {
        self.state.create_status.store(status, Ordering::SeqCst);
    }

    pub fn delete_status(&self, status: u16) {
        self.state.delete_status.store(status, Ordering::SeqCst);
    }

    pub fn stop_status(&self, status: u16) {
        self.state.stop_status.store(status, Ordering::SeqCst);
    }

    pub fn get_status(&self, status: u16) {
        self.state.get_status.store(status, Ordering::SeqCst);
    }

    pub fn create_delay(&self, delay: Duration) {
        let millis = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        self.state.create_delay_ms.store(millis, Ordering::SeqCst);
    }

    pub fn read_content(&self, content: Vec<u8>) {
        *self
            .state
            .read_content
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = content;
    }

    pub fn written_bytes(&self) -> usize {
        self.state.write_bytes.load(Ordering::SeqCst)
    }

    pub fn provider(&self) -> Result<Arc<VercelWorkerProvider>, Boxed> {
        Ok(Arc::new(VercelWorkerProvider::new(
            self.server.api()?,
            "node24",
            8,
            VercelPlan::Pro,
            Arc::new(self.server.exec()?),
        )))
    }
}

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

    pub async fn client(&self) -> Client {
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
    pub provider: Arc<VercelWorkerProvider>,
    pub worker_id: WorkerId,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), VercelWorkerError>>>,
}

pub fn request_json() -> Value {
    json!({
        "tenant": "test",
        "provider_pref": ["vercel"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": {"registry": "docker.io", "repository": "library/ubuntu", "tag": "24.04"},
            },
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 60,
        "idempotency_key": "test",
    })
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
        let fake = Fake::start().await?;
        let provider = fake.provider()?;
        let worker_id = WorkerId::new("worker-1")?;
        if let Some(path) = ledger {
            provider.configure_dispatch_ledger(Arc::new(trogon_sandbox_protocol::AttemptLedger::open(
                path,
                trogon_sandbox_contract::ProviderKind::Vercel,
                Some(worker_id.clone()),
            )?))?;
        }
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (client, provider, worker_id, cancel) =
                (client.clone(), provider.clone(), worker_id.clone(), cancel.clone());
            async move {
                let js = jetstream::new(client.clone());
                trogon_sandbox_vercel_worker::run(client, js, provider, &worker_id, cancel).await
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

    /// The worker subscribes to the lease subject last but one, so an answer there means the verbs are served too.
    async fn wait_until_serving(&self) -> Result<(), Boxed> {
        for _ in 0..100 {
            let probe = self.client.request("sandbox.provider.vercel.lease", "{}".into());
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
        self.ask("sandbox.provider.vercel.lease", &dispatch_json(lease)).await
    }

    /// Sends a lease verb such as `run_command` or `pause`.
    pub async fn verb(&self, verb: &str, lease: &str, mut payload: Value) -> Result<Value, Boxed> {
        payload["lease_id"] = json!(lease);
        self.ask(&format!("sandbox.provider.vercel.{verb}.{lease}"), &payload)
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
        self.publish(format!("sandbox.provider.vercel.cancel.{lease}"), &json!({}))
            .await
    }
}

/// Polls until `check` holds.
pub async fn until(mut check: impl FnMut() -> bool) -> Result<(), Boxed> {
    for _ in 0..100 {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("condition not reached".into())
}
