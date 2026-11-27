use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::Message;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use serde::{Serialize, Serializer};
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, LifecycleEvent, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, ProviderLease,
    SandboxId, WorkerId,
};
use trogon_sandbox_protocol::{Provider, ProviderError, RedispatchGuard, lifecycle_subject};

use super::super::{handle_lease_msg, publish_exit_marker, publish_json};
use super::support::{TestResult, WAIT, create_evidence_stream};

/// A provider that counts launches and either grants every lease or refuses it.
struct ScriptedProvider {
    launches: AtomicUsize,
    refuse: bool,
}

impl ScriptedProvider {
    fn new(refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            launches: AtomicUsize::new(0),
            refuse,
        })
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Substrate
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        if self.refuse {
            return Err(ProviderError::backend("the control plane is down"));
        }
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(lease_id.as_str()).map_err(ProviderError::backend)?,
            evidence_subject: format!("sandbox.evidence.{lease_id}"),
            worker_id: Some(WorkerId::new("worker-1").map_err(ProviderError::backend)?),
            attempt_id: None,
            epoch: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// A value that cannot be serialized.
struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("refused"))
    }
}

fn payload(lease: &str) -> Result<ProviderDispatchPayload, serde_json::Error> {
    Ok(ProviderDispatchPayload {
        lease_id: LeaseId::new(lease),
        request: serde_json::from_value(json!({
            "tenant": "acme",
            "provider_pref": ["substrate"],
            "spec": {
                "image": {"kind": "registry", "reference": {"registry": "", "repository": "agent"}},
                "workspace": {"empty": {}},
                "cpu_seconds": 1,
                "memory_mib": 64,
            },
            "ttl_seconds": 60,
            "idempotency_key": "k",
        }))?,
        correlation_id: None,
        resolved_drives: Vec::new(),
        target: None,
    })
}

fn dispatch_message(reply: Option<String>, payload: Bytes) -> Message {
    Message {
        subject: "sandbox.provider.substrate.lease".into(),
        reply: reply.map(Into::into),
        length: payload.len(),
        payload,
        headers: None,
        status: None,
        description: None,
    }
}

struct Harness {
    _server: JetStreamTestServer,
    client: async_nats::Client,
    js: async_nats::jetstream::Context,
    grants: Arc<RedispatchGuard>,
}

impl Harness {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let js = server.jetstream().await;
        create_evidence_stream(&js).await?;
        Ok(Self {
            _server: server,
            client,
            js,
            grants: Arc::new(RedispatchGuard::new()),
        })
    }

    /// Delivers `payload` as a dispatch and returns the reply it produced.
    async fn dispatch(
        &self,
        provider: &Arc<ScriptedProvider>,
        payload: Bytes,
    ) -> Result<ProviderDispatchReply, Box<dyn std::error::Error>> {
        let inbox = self.client.new_inbox();
        let mut replies = self.client.subscribe(inbox.clone()).await?;
        handle_lease_msg(
            dispatch_message(Some(inbox), payload),
            self.client.clone(),
            self.js.clone(),
            provider.clone(),
            self.grants.clone(),
        )
        .await?;
        let reply = tokio::time::timeout(WAIT, replies.next()).await?.ok_or("no reply")?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// The lifecycle events published for `lease`, in order.
    async fn lifecycle(&self, lease: &str) -> Result<Vec<LifecycleEvent>, Box<dyn std::error::Error>> {
        let consumer = self
            .js
            .get_stream("EVIDENCE")
            .await?
            .create_consumer(async_nats::jetstream::consumer::pull::Config {
                filter_subject: lifecycle_subject(&LeaseId::new(lease)),
                ..Default::default()
            })
            .await?;
        let mut messages = consumer.fetch().expires(Duration::from_millis(500)).messages().await?;
        let mut events = Vec::new();
        while let Some(message) = messages.next().await {
            let message = message.map_err(|error| error.to_string())?;
            events.push(serde_json::from_slice(&message.payload)?);
        }
        Ok(events)
    }
}

#[tokio::test]
async fn a_redispatched_lease_launches_and_announces_started_once() -> TestResult {
    let harness = Harness::start().await?;
    let provider = ScriptedProvider::new(false);
    let body = Bytes::from(serde_json::to_vec(&payload("redispatch-lease")?)?);

    let first = harness.dispatch(&provider, body.clone()).await?;
    let second = harness.dispatch(&provider, body).await?;

    assert!(matches!(first, ProviderDispatchReply::Granted(_)));
    assert!(matches!(second, ProviderDispatchReply::Granted(_)));
    assert_eq!(provider.launches.load(Ordering::SeqCst), 1);
    let events = harness.lifecycle("redispatch-lease").await?;
    let started = events
        .iter()
        .filter(|event| matches!(event, LifecycleEvent::Started { .. }))
        .count();
    assert_eq!(started, 1, "a redelivery must not publish a second Started");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LifecycleEvent::Completed { .. }))
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_launch_is_reported_and_closes_the_lifecycle_with_failed() -> TestResult {
    let harness = Harness::start().await?;
    let provider = ScriptedProvider::new(true);
    let body = Bytes::from(serde_json::to_vec(&payload("failing-lease")?)?);

    let reply = harness.dispatch(&provider, body).await?;

    assert!(matches!(reply, ProviderDispatchReply::LaunchFailed { .. }));
    let events = harness.lifecycle("failing-lease").await?;
    assert!(matches!(
        events.as_slice(),
        [LifecycleEvent::Started { .. }, LifecycleEvent::Failed { .. }]
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_id_that_is_not_a_sandbox_id_launches_without_a_lifecycle() -> TestResult {
    let harness = Harness::start().await?;
    let provider = ScriptedProvider::new(true);
    let body = Bytes::from(serde_json::to_vec(&payload("not\u{1}a sandbox id")?)?);

    let reply = harness.dispatch(&provider, body).await?;

    assert!(matches!(reply, ProviderDispatchReply::LaunchFailed { .. }));
    assert_eq!(provider.launches.load(Ordering::SeqCst), 1);
    assert!(
        harness
            .lifecycle("not\u{1}a sandbox id")
            .await
            .map_or(true, |events| events.is_empty())
    );
    Ok(())
}

#[tokio::test]
async fn an_undecodable_dispatch_is_rejected_without_launching() -> TestResult {
    let harness = Harness::start().await?;
    let provider = ScriptedProvider::new(false);

    let reply = harness.dispatch(&provider, Bytes::from_static(b"nope")).await?;

    assert!(matches!(reply, ProviderDispatchReply::DispatchRejected { .. }));
    assert_eq!(provider.launches.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_dispatch_nobody_waits_on_still_launches() -> TestResult {
    let harness = Harness::start().await?;
    let provider = ScriptedProvider::new(false);
    let body = Bytes::from(serde_json::to_vec(&payload("fire-and-forget")?)?);

    handle_lease_msg(
        dispatch_message(None, body),
        harness.client.clone(),
        harness.js.clone(),
        provider.clone(),
        harness.grants.clone(),
    )
    .await?;

    assert_eq!(provider.launches.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn evidence_that_cannot_be_encoded_or_published_is_dropped() -> TestResult {
    let harness = Harness::start().await?;

    publish_json(
        &harness.js,
        "sandbox.evidence.unserializable",
        &Unserializable,
        "test value",
    )
    .await;
    let oversized = "x".repeat(2 * 1024 * 1024);
    publish_json(&harness.js, "sandbox.evidence.oversized", &oversized, "test value").await;
    Ok(())
}

#[tokio::test]
async fn an_exit_marker_that_cannot_be_published_is_dropped() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    publish_exit_marker(
        &js,
        &LeaseId::new("l1"),
        &trogon_sandbox_contract::ExecutionId::new("e1"),
        Some(0),
    )
    .await;
    Ok(())
}
