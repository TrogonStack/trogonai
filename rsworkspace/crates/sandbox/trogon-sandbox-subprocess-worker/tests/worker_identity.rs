#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::error::Error;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderDispatchPayload, ProviderKind, ProviderLease, WorkerHeartbeat};
use trogon_sandbox_protocol::subjects;

type TestResult = Result<(), Box<dyn Error>>;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker");
const STEP: Duration = Duration::from_secs(20);

struct OwnedWorker(Child);

impl Drop for OwnedWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn dispatch(lease: &str) -> Result<ProviderDispatchPayload, serde_json::Error> {
    serde_json::from_value(json!({
        "lease_id": lease,
        "request": {
            "tenant": "worker-identity",
            "provider_pref": ["subprocess"],
            "spec": {
                "image": {
                    "kind": "registry",
                    "reference": { "registry": "", "repository": "/usr/bin/true" },
                },
                "workspace": { "empty": {} },
                "cpu_seconds": 2,
                "memory_mib": 512,
                "env": {},
                "mounts": [],
            },
            "ttl_seconds": 30,
            "idempotency_key": "worker-identity",
        },
    }))
}

async fn grant(
    client: &async_nats::Client,
    payload: &ProviderDispatchPayload,
) -> Result<ProviderLease, Box<dyn Error>> {
    let body = serde_json::to_vec(payload)?;
    tokio::time::timeout(STEP, async {
        loop {
            match client
                .request(
                    subjects::provider_dispatch(ProviderKind::Subprocess),
                    body.clone().into(),
                )
                .await
            {
                Ok(reply) => return Ok::<_, Box<dyn Error>>(serde_json::from_slice(&reply.payload)?),
                Err(error) if error.kind() == async_nats::client::RequestErrorKind::NoResponders => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await?
}

#[tokio::test]
async fn authority_grant_matches_the_actual_worker_binary_heartbeat_identity() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let root = tempfile::tempdir()?;
    let log_path = root.path().join("worker.log");
    let log = std::fs::File::create(&log_path)?;
    let mut heartbeats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Subprocess))
        .await?;
    client.flush().await?;
    let worker = OwnedWorker(
        Command::new(BINARY)
            .arg("--nats-url")
            .arg(format!("nats://{}", nats.address()))
            .args(["--agent-binary", "/usr/bin/true", "--isolation-backend", "none"])
            .args(["--metrics-addr", "127.0.0.1:0"])
            .arg("--workspace-root")
            .arg(root.path())
            .env("SANDBOX_WORKER_AUTHORITY", "off")
            .env("SANDBOX_FUSE_ENABLED", "false")
            .env("SANDBOX_REQUIRE_COMMITTED_POLICY", "false")
            .env_remove("SANDBOX_OTLP_ENDPOINT")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?,
    );
    let observation: Result<(WorkerHeartbeat, ProviderLease), Box<dyn Error>> = async {
        let message = tokio::time::timeout(STEP, heartbeats.next())
            .await?
            .ok_or("owned worker heartbeat feed closed")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        let lease = format!("worker-identity-{}", uuid::Uuid::now_v7().simple());
        let granted = grant(&client, &dispatch(&lease)?).await?;
        Ok((heartbeat, granted))
    }
    .await;
    drop(worker);
    let (heartbeat, granted) = observation.map_err(|error| {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        format!("{error}; worker log: {log}")
    })?;
    assert_eq!(
        granted.worker_id,
        Some(heartbeat.worker_id),
        "the actual CLI must hand its heartbeat/authority identity to lease replies"
    );
    Ok(())
}
