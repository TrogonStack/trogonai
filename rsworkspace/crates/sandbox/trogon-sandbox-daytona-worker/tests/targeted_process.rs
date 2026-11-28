//! The worker binary with a dispatch ledger and required dispatch signatures: it announces targeted dispatch, answers
//! a signed dispatch once per attempt, refuses a cleanup signed for another assignment, and confirms a cleanup only
//! once the sandbox is deleted, so a failed delete is retried.

#![cfg(unix)]

mod support;

use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use support::{Boxed, Fake, SANDBOX, TestResult, key, request_json};
use tokio::process::Command;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    DispatchAttemptId, LeaseEpoch, LeaseId, ProviderKind, TenantId, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope, subjects};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-daytona-worker");
const LEASE: &str = "lease-signed";
const ATTEMPT: &str = "attempt-a";
/// The tenant `request_json` leases for, which the dispatch fence binds the lease to.
const TENANT: &str = "t";
const WAIT: Duration = Duration::from_secs(60);

fn sandbox_route() -> String {
    format!("/sandbox/{SANDBOX}")
}

struct Worker {
    client: async_nats::Client,
    signer: DispatchSigner,
    worker_id: WorkerId,
}

impl Worker {
    fn target(&self, attempt: &str) -> Value {
        json!({"worker_id": self.worker_id.as_str(), "attempt_id": attempt, "epoch": 1})
    }

    fn scope(&self, attempt: &str) -> Result<VerbScope, Boxed> {
        Ok(VerbScope {
            tenant: TenantId::new(TENANT)?,
            lease_id: LeaseId::new(LEASE),
            worker_id: Some(self.worker_id.clone()),
            attempt_id: Some(DispatchAttemptId::new(attempt)?),
            epoch: Some(LeaseEpoch::new(1)?),
        })
    }

    /// Sends a signed dispatch once, without retrying: a worker that heartbeats must already serve its dispatch subject.
    async fn dispatch(&self) -> Result<Value, Boxed> {
        let body = json!({"lease_id": LEASE, "request": request_json(), "target": self.target(ATTEMPT)});
        let signed = self
            .signer
            .sign_payload(&serde_json::to_vec(&body)?, OP_LEASE_DISPATCH)?;
        let subject = subjects::worker_dispatch(ProviderKind::Daytona, &self.worker_id);
        let reply = tokio::time::timeout(WAIT, self.client.request(subject, signed.into())).await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// Sends a cleanup for `attempt`, signed for the attempt `signed_for`.
    async fn cleanup(&self, attempt: &str, signed_for: &str) -> Result<Value, Boxed> {
        let subject = subjects::worker_cleanup(ProviderKind::Daytona, &self.worker_id);
        let body = serde_json::to_vec(&json!({
            "identity": {"lease_id": LEASE, "provider": "daytona", "target": self.target(attempt)},
            "intent": "release",
        }))?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(
            AUTH_HEADER,
            self.signer.sign_verb(&subject, &body, &self.scope(signed_for)?)?,
        );
        let reply =
            tokio::time::timeout(WAIT, self.client.request_with_headers(subject, headers, body.into())).await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_targeted_lease_is_dispatched_once_and_cleaned_up_only_when_exactly_signed() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let fake = Fake::with_sandbox().await?;
    // The binary keeps snapshot custody, which confirms the account owning a sandbox before deleting it.
    fake.json(
        "GET",
        &sandbox_route(),
        200,
        json!({"id": SANDBOX, "state": "started", "target": "us", "organizationId": "org-1"}),
    );
    let directory = tempfile::tempdir()?;
    let signer = DispatchSigner::new("daytona-process", [7; 32]);
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Daytona))
        .await?;
    client.flush().await?;
    let _child = Command::new(BINARY)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("DAYTONA_API_KEY", key())
        .env("RUST_LOG", "off")
        .env("SANDBOX_NATS_URL", format!("nats://{}", nats.address()))
        .env("DAYTONA_BASE_URL", &fake.url)
        .env(
            "SANDBOX_DAYTONA_DISPATCH_LEDGER_FILE",
            directory.path().join("ledger.sqlite3"),
        )
        .env("SANDBOX_DISPATCH_VERIFY", "require")
        .env(
            "SANDBOX_DISPATCH_PUBLIC_KEYS",
            format!("{}:{}", signer.key_id(), signer.public_key_b64()),
        )
        .args(["--metrics-addr", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let beat = tokio::time::timeout(WAIT, beats.next()).await?.ok_or("no heartbeat")?;
    let beat: WorkerHeartbeat = serde_json::from_slice(&beat.payload)?;
    assert_eq!(beat.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
    assert!(beat.supports_lease_epoch.is_supported());
    let worker = Worker {
        client,
        signer,
        worker_id: beat.worker_id,
    };

    let granted = worker.dispatch().await?;
    let again = worker.dispatch().await?;
    assert_eq!(granted, again);
    assert_eq!(granted["attempt_id"], ATTEMPT, "{granted}");
    assert_eq!(fake.count("POST", "/sandbox"), 1);

    // The dispatch fence refuses a request signed for an attempt that is not the live one before the cleanup handler
    // sees it; the handler's own exact-scope check stands behind it.
    let foreign = worker.cleanup(ATTEMPT, "attempt-other").await?;
    assert_eq!(foreign["error"], "dispatch_auth", "{foreign}");
    assert_eq!(
        fake.count("DELETE", &sandbox_route()),
        0,
        "a cleanup signed for another attempt deletes nothing"
    );

    fake.json("DELETE", &sandbox_route(), 500, json!({"message": "boom"}));
    let failed = worker.cleanup(ATTEMPT, ATTEMPT).await?;
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert_eq!(fake.count("DELETE", &sandbox_route()), 1, "{failed}");

    fake.json("DELETE", &sandbox_route(), 200, json!({}));
    let cleaned = worker.cleanup(ATTEMPT, ATTEMPT).await?;
    assert_eq!(cleaned["outcome"], "cleaned", "{cleaned}");
    assert_eq!(fake.count("DELETE", &sandbox_route()), 2);
    Ok(())
}
