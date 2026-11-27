//! A worker with snapshot custody, served over a throwaway NATS server against the fake Daytona, and the scripted
//! answers a native snapshot of the sandbox `SANDBOX` needs.

#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::Arc;
use std::time::Duration;

use async_nats::Client;
use async_nats::jetstream;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{SnapshotExportOutcome, WorkerId};
use trogon_sandbox_daytona_worker::{DaytonaWorkerError, DaytonaWorkerProvider, SnapshotCustody, SnapshotWait, Wait};
use trogon_sandbox_protocol::nats_transfer;

use crate::nats_support::Nats;
use crate::support::{Boxed, Fake, SANDBOX, lease_id, request};

pub const LEASE: &str = "lease-1";
pub const ORGANIZATION: &str = "org-1";
pub const SNAPSHOT_ID: &str = "snap-1";
pub const NAME: &str = "trogon-abc";

/// What Daytona returns for `SANDBOX`, owned by `ORGANIZATION`.
pub fn sandbox() -> Value {
    json!({"id": SANDBOX, "state": "started", "target": "us", "organizationId": ORGANIZATION})
}

/// What Daytona returns for a snapshot of `SANDBOX`.
pub fn snapshot(name: &str, state: &str) -> Value {
    snapshot_of(name, state, SANDBOX, ORGANIZATION)
}

pub fn snapshot_of(name: &str, state: &str, source: &str, organization: &str) -> Value {
    json!({
        "id": SNAPSHOT_ID, "name": name, "state": state, "general": false, "size": 2.5,
        "sourceSandboxId": source, "organizationId": organization,
    })
}

pub fn snapshot_route(name: &str) -> String {
    format!("/snapshots/{name}")
}

pub fn capture_route() -> String {
    format!("/sandbox/{SANDBOX}/snapshot")
}

/// A fake that also knows the sandbox's owner and accepts a capture.
pub async fn fake() -> Result<Fake, Boxed> {
    let fake = Fake::with_sandbox().await?;
    fake.json("GET", &format!("/sandbox/{SANDBOX}"), 200, sandbox());
    fake.json("POST", &capture_route(), 200, sandbox());
    Ok(fake)
}

/// A worker serving a provider that holds `LEASE`, on a NATS server until stopped.
pub struct Served {
    pub nats: Nats,
    pub client: Client,
    pub fake: Fake,
    pub provider: Arc<DaytonaWorkerProvider>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), DaytonaWorkerError>>>,
}

impl Served {
    /// A worker whose snapshots wait `wait`, with custody unless `custody` is false, and `LEASE` already leased.
    pub async fn start(fake: Fake, wait: SnapshotWait, custody: bool) -> Result<Self, Boxed> {
        let nats = Nats::start().await;
        let client = nats.client().await;
        let js = jetstream::new(client.clone());
        let mut provider = DaytonaWorkerProvider::new(fake.api()?, "base:1", Arc::new(fake.exec()?))
            .with_wait(Wait {
                timeout: Duration::from_secs(20),
                interval: Duration::from_millis(10),
            })
            .with_snapshot_wait(wait);
        if custody {
            provider = provider.with_snapshot_custody(SnapshotCustody::new(js.clone(), &fake.api()?));
        }
        let provider = Arc::new(provider);
        let worker_id = WorkerId::new("worker-1")?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (client, provider, cancel) = (client.clone(), provider.clone(), cancel.clone());
            async move { trogon_sandbox_daytona_worker::run(client, js, provider, &worker_id, cancel).await }
        });
        let served = Self {
            nats,
            client,
            fake,
            provider,
            cancel,
            task: Some(task),
        };
        served.wait_until_serving().await?;
        use_lease(&served.provider).await?;
        Ok(served)
    }

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

    /// Asks the worker for a native snapshot of the leased sandbox under `name`.
    pub async fn capture(&self, name: &str) -> Result<SnapshotExportOutcome, Boxed> {
        self.capture_for(LEASE, name).await
    }

    /// Asks the worker for a native snapshot of the sandbox `lease` holds, under `name`.
    pub async fn capture_for(&self, lease: &str, name: &str) -> Result<SnapshotExportOutcome, Boxed> {
        let payload = json!({
            "lease_id": lease,
            "target": {"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"},
            "native_name": name,
        });
        let reply = nats_transfer::request(
            &self.client,
            &format!("sandbox.provider.daytona.checkpoint_running.{lease}"),
            serde_json::to_vec(&payload)?.into(),
            None,
            Duration::from_secs(30),
        )
        .await?;
        Ok(serde_json::from_slice(&reply)?)
    }

    /// The custody record for `SANDBOX` as stored in the JetStream bucket, or `None` when custody never wrote one.
    pub async fn custody_record(&self) -> Result<Option<Value>, Boxed> {
        let origin = hex::encode(Sha256::digest(self.fake.api()?.config().endpoint().as_str().as_bytes()));
        let store = jetstream::new(self.client.clone())
            .create_or_update_key_value(jetstream::kv::Config {
                bucket: "sandbox-daytona-snapshot-custody".into(),
                history: 1,
                ..Default::default()
            })
            .await?;
        match store.get(format!("{origin}.{SANDBOX}")).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// A provider that shares this worker's custody bucket and Daytona but none of its in-memory state, as a worker
    /// that restarted would.
    pub fn restarted_provider(&self) -> Result<DaytonaWorkerProvider, Boxed> {
        let js = jetstream::new(self.client.clone());
        Ok(
            DaytonaWorkerProvider::new(self.fake.api()?, "base:1", Arc::new(self.fake.exec()?))
                .with_snapshot_custody(SnapshotCustody::new(js, &self.fake.api()?)),
        )
    }

    pub async fn stop(&mut self) -> Result<(), Boxed> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

async fn use_lease(provider: &DaytonaWorkerProvider) -> Result<(), Boxed> {
    trogon_sandbox_protocol::Provider::lease(provider, &request()?, &lease_id(LEASE)).await?;
    Ok(())
}

pub fn quick(timeout_ms: u64) -> SnapshotWait {
    SnapshotWait {
        poll_interval: Duration::from_millis(5),
        timeout: Duration::from_millis(timeout_ms),
    }
}
