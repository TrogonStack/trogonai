use std::sync::atomic::{AtomicUsize, Ordering};

use async_nats::{Client, Message, Subject};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Counting {
    leases: AtomicUsize,
    fail: bool,
}

impl Counting {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            leases: AtomicUsize::new(0),
            fail,
        })
    }
}

#[async_trait]
impl Provider for Counting {
    fn kind(&self) -> ProviderKind {
        ProviderKind::E2b
    }

    async fn lease(&self, _req: &LeaseRequest, _lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.leases.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(ProviderError::backend("e2b unavailable"));
        }
        Ok(ProviderLease {
            sandbox_id: SandboxId::new("sandbox-1").map_err(ProviderError::backend)?,
            evidence_subject: "sandbox.evidence.lease-1.>".to_owned(),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn request_json() -> Value {
    json!({
        "tenant": "test",
        "provider_pref": ["e2b"],
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

struct Harness {
    _server: JetStreamTestServer,
    client: Client,
    js: jetstream::Context,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
}

impl Harness {
    async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        Self {
            js: jetstream::new(client.clone()),
            client,
            _server: server,
            active: ActiveLeases::default(),
            grants: Arc::new(RedispatchGuard::new()),
        }
    }

    /// Delivers `payload` as a dispatch and returns what the handler replied.
    async fn dispatch(&self, provider: &Arc<Counting>, payload: Vec<u8>) -> Result<Value, Box<dyn std::error::Error>> {
        let inbox = self.client.new_inbox();
        let mut replies = self.client.subscribe(inbox.clone()).await?;
        self.client.flush().await?;
        let message = Message {
            subject: Subject::from("sandbox.provider.e2b.lease"),
            reply: Some(Subject::from(inbox)),
            payload: payload.into(),
            headers: None,
            status: None,
            description: None,
            length: 0,
        };
        let provider: Arc<dyn Provider> = provider.clone();
        handle_lease_msg(
            message,
            self.client.clone(),
            self.js.clone(),
            provider,
            Arc::clone(&self.active),
            Arc::clone(&self.grants),
        )
        .await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(10), replies.next())
            .await?
            .ok_or("no reply")?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }
}

fn dispatch_bytes(lease: &str, target: Option<Value>) -> Result<Vec<u8>, serde_json::Error> {
    let mut body = json!({"lease_id": lease, "request": request_json()});
    if let Some(target) = target {
        body["target"] = target;
    }
    serde_json::to_vec(&body)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redispatched_lease_id_launches_once() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    let first = harness.dispatch(&provider, dispatch_bytes("lease-1", None)?).await?;
    let second = harness.dispatch(&provider, dispatch_bytes("lease-1", None)?).await?;
    assert_eq!(first, second);
    assert_eq!(first["sandbox_id"], "sandbox-1");
    assert_eq!(provider.leases.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forgotten_lease_id_launches_again_when_reused() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    harness.dispatch(&provider, dispatch_bytes("lease-1", None)?).await?;
    harness.grants.forget(&LeaseId::new("lease-1"));
    harness.dispatch(&provider, dispatch_bytes("lease-1", None)?).await?;
    assert_eq!(provider.leases.load(Ordering::SeqCst), 2);
    assert!(harness.active.lock().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_launch_replies_with_the_reason() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(true);
    let reply = harness.dispatch(&provider, dispatch_bytes("lease-1", None)?).await?;
    assert_eq!(reply["provider_error"], "e2b unavailable");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_that_does_not_decode_is_rejected() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    let reply = harness.dispatch(&provider, b"{}".to_vec()).await?;
    assert!(reply["decode_error"].is_string());
    assert_eq!(provider.leases.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_assigned_dispatch_on_the_shared_intake_is_rejected() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    let target = json!({"worker_id": "worker-1", "attempt_id": "attempt-1"});
    let reply = harness
        .dispatch(&provider, dispatch_bytes("lease-1", Some(target))?)
        .await?;
    assert_eq!(reply["decode_error"], ASSIGNED_ON_SHARED_INTAKE);
    assert_eq!(provider.leases.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    let shared: Arc<dyn Provider> = provider.clone();
    handle_lease_msg(
        bare_message(dispatch_bytes("lease-1", None)?),
        harness.client.clone(),
        harness.js.clone(),
        shared,
        Arc::clone(&harness.active),
        Arc::clone(&harness.grants),
    )
    .await?;
    assert_eq!(provider.leases.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_dispatch_without_a_reply_subject_is_dropped() -> TestResult {
    let harness = Harness::start().await;
    let provider = Counting::new(false);
    let shared: Arc<dyn Provider> = provider.clone();
    handle_lease_msg(
        bare_message(b"{}".to_vec()),
        harness.client.clone(),
        harness.js.clone(),
        shared,
        Arc::clone(&harness.active),
        Arc::clone(&harness.grants),
    )
    .await?;
    assert_eq!(provider.leases.load(Ordering::SeqCst), 0);
    Ok(())
}

fn bare_message(payload: Vec<u8>) -> Message {
    Message {
        subject: Subject::from("sandbox.provider.e2b.lease"),
        reply: None,
        payload: payload.into(),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}
