mod support;

use std::sync::Arc;
use std::time::Duration;

use async_nats::Client;
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseError, LeaseId, ProviderKind, ProviderLease, RunCommandResponse, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch};
use trogon_sandbox_v8_wasm_worker::{RoutedVerb, V8WasmProvider, WorkerError, run_worker};

use support::TestResult;
use support::lease::Lease;
use support::mock_provider::{FAILING_TENANT, MockProvider, UNCANCELLABLE_LEASE};

const REPLY_WAIT: Duration = Duration::from_secs(10);

struct Harness {
    _server: JetStreamTestServer,
    client: Client,
    cancel: CancellationToken,
    worker: JoinHandle<Result<(), WorkerError>>,
    provider: Arc<MockProvider>,
    v8: Arc<V8WasmProvider>,
}

impl Harness {
    /// A worker whose leases run on a recording provider instead of the V8 provider.
    async fn external() -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let provider = Arc::new(MockProvider::default());
        let v8 = Arc::new(V8WasmProvider::new(server.jetstream().await));
        let cancel = CancellationToken::new();
        let worker = tokio::spawn(run_worker(
            client.clone(),
            provider.clone(),
            v8.clone(),
            WorkerId::new("worker-under-test")?,
            cancel.clone(),
        ));
        let harness = Self {
            _server: server,
            client,
            cancel,
            worker,
            provider,
            v8,
        };
        harness.wait_until_subscribed().await?;
        Ok(harness)
    }

    /// A worker whose leases run on the real V8 provider.
    async fn native() -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let v8 = Arc::new(V8WasmProvider::new(server.jetstream().await));
        let cancel = CancellationToken::new();
        let worker = tokio::spawn(run_worker(
            client.clone(),
            v8.clone(),
            v8.clone(),
            WorkerId::new("worker-under-test")?,
            cancel.clone(),
        ));
        let harness = Self {
            _server: server,
            client,
            cancel,
            worker,
            provider: Arc::new(MockProvider::default()),
            v8,
        };
        harness.wait_until_subscribed().await?;
        Ok(harness)
    }

    /// The worker subscribes to the last verb subject last, so a subscriber there means all are up.
    async fn wait_until_subscribed(&self) -> TestResult {
        let probe = verb_subject(RoutedVerb::GetDomain, "probe");
        tokio::time::timeout(REPLY_WAIT, async {
            // An unknown lease is never answered, so a probe that times out means the subscription exists.
            while let Ok(answer) = tokio::time::timeout(
                Duration::from_millis(300),
                self.client.request(probe.clone(), Bytes::new()),
            )
            .await
            {
                assert!(answer.is_err(), "unexpected answer to the readiness probe");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        Ok(())
    }

    async fn request(&self, subject: String, body: Vec<u8>) -> Result<async_nats::Message, Box<dyn std::error::Error>> {
        let reply = tokio::time::timeout(REPLY_WAIT, self.client.request(subject, Bytes::from(body))).await??;
        Ok(reply)
    }

    async fn dispatch(&self, lease: &str, request: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let body = serde_json::to_vec(&json!({ "lease_id": lease, "request": request }))?;
        let reply = self.request(provider_dispatch(ProviderKind::V8Wasm), body).await?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn stop(self) -> TestResult {
        self.cancel.cancel();
        tokio::time::timeout(REPLY_WAIT, self.worker).await???;
        Ok(())
    }
}

fn verb_subject(verb: RoutedVerb, lease: &str) -> String {
    verb.subject().replace('>', lease)
}

fn plain_lease(tenant: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let mut request = Lease::new(json!({ "empty": {} }), "js").json();
    request["tenant"] = json!(tenant);
    Ok(request)
}

async fn eventually(mut done: impl FnMut() -> bool) -> TestResult {
    tokio::time::timeout(REPLY_WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_dispatched_lease_replies_with_the_grant_stamped_with_the_worker() -> TestResult {
    let harness = Harness::external().await?;

    let reply = harness.dispatch("lease-1", plain_lease("acme")?).await?;

    let grant: ProviderLease = serde_json::from_value(reply)?;
    assert_eq!(grant.sandbox_id.as_str(), "lease-1");
    assert_eq!(grant.worker_id, Some(WorkerId::new("worker-under-test")?));
    assert_eq!(harness.provider.leased(), vec!["lease-1".to_owned()]);
    harness.stop().await
}

#[tokio::test]
async fn a_redispatched_lease_launches_once_until_it_is_cancelled() -> TestResult {
    let harness = Harness::external().await?;
    let lease = LeaseId::new("lease-redispatch");

    let first = harness.dispatch(lease.as_str(), plain_lease("acme")?).await?;
    let second = harness.dispatch(lease.as_str(), plain_lease("acme")?).await?;
    assert_eq!(first, second);
    assert_eq!(harness.provider.leased().len(), 1);

    harness
        .client
        .publish(provider_cancel(ProviderKind::V8Wasm, &lease), Bytes::new())
        .await?;
    eventually(|| harness.provider.cancelled() == vec!["lease-redispatch".to_owned()]).await?;
    // The grant is forgotten only after the cancel settled, so wait for a relaunch to take effect.
    eventually(|| harness.provider.cancelled().len() == 1).await?;
    harness.dispatch(lease.as_str(), plain_lease("acme")?).await?;
    assert_eq!(harness.provider.leased().len(), 2);
    harness.stop().await
}

#[tokio::test]
async fn a_failed_launch_is_reported_and_retried() -> TestResult {
    let harness = Harness::external().await?;

    let first = harness.dispatch("lease-fails", plain_lease(FAILING_TENANT)?).await?;
    harness.dispatch("lease-fails", plain_lease(FAILING_TENANT)?).await?;

    assert!(
        first["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("synthetic"))
    );
    assert_eq!(harness.provider.leased().len(), 2);
    harness.stop().await
}

#[tokio::test]
async fn a_malformed_dispatch_is_answered_with_a_decode_error() -> TestResult {
    let harness = Harness::external().await?;

    let reply = harness
        .request(provider_dispatch(ProviderKind::V8Wasm), b"not json".to_vec())
        .await?;

    let body: Value = serde_json::from_slice(&reply.payload)?;
    assert!(body.get("decode_error").is_some(), "{body}");
    assert!(harness.provider.leased().is_empty());
    harness.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let harness = Harness::external().await?;
    let body = serde_json::to_vec(&json!({ "lease_id": "lease-silent", "request": plain_lease("acme")? }))?;

    harness
        .client
        .publish(provider_dispatch(ProviderKind::V8Wasm), Bytes::from(body))
        .await?;
    harness
        .client
        .publish(provider_dispatch(ProviderKind::V8Wasm), Bytes::from_static(b"garbage"))
        .await?;

    eventually(|| harness.provider.leased() == vec!["lease-silent".to_owned()]).await?;
    harness.stop().await
}

#[tokio::test]
async fn a_failed_cancel_is_logged_and_keeps_the_grant() -> TestResult {
    let harness = Harness::external().await?;
    let lease = LeaseId::new(UNCANCELLABLE_LEASE);
    harness.dispatch(lease.as_str(), plain_lease("acme")?).await?;

    harness
        .client
        .publish(provider_cancel(ProviderKind::V8Wasm, &lease), Bytes::new())
        .await?;
    eventually(|| harness.provider.cancelled().len() == 1).await?;
    harness.dispatch(lease.as_str(), plain_lease("acme")?).await?;

    assert_eq!(harness.provider.leased().len(), 1);
    harness.stop().await
}

#[tokio::test]
async fn every_routed_verb_of_a_held_lease_gets_its_typed_unsupported_reply() -> TestResult {
    let harness = Harness::external().await?;
    harness.dispatch("lease-verbs", plain_lease("acme")?).await?;

    for verb in RoutedVerb::ALL {
        let reply = harness
            .request(verb_subject(verb, "lease-verbs"), b"{}".to_vec())
            .await?;
        assert_eq!(reply.payload.to_vec(), verb.unsupported_reply()?, "{verb:?}");
    }
    harness.stop().await
}

#[tokio::test]
async fn routed_verbs_for_leases_this_worker_does_not_hold_get_no_reply() -> TestResult {
    let harness = Harness::external().await?;

    let unknown = tokio::time::timeout(
        Duration::from_millis(300),
        harness
            .client
            .request(verb_subject(RoutedVerb::ReadFile, "someone-elses"), Bytes::new()),
    )
    .await;
    harness
        .client
        .publish(verb_subject(RoutedVerb::ReadFile, "someone-elses"), Bytes::new())
        .await?;
    harness
        .client
        .publish("sandbox.provider.v8_wasm.read_file", Bytes::new())
        .await?;

    assert!(unknown.is_err(), "an unknown lease must not be answered");
    harness.stop().await
}

#[tokio::test]
async fn a_routed_verb_sent_without_a_reply_subject_is_ignored() -> TestResult {
    let harness = Harness::external().await?;
    harness.dispatch("lease-fire-and-forget", plain_lease("acme")?).await?;

    harness
        .client
        .publish(
            verb_subject(RoutedVerb::WriteFiles, "lease-fire-and-forget"),
            Bytes::new(),
        )
        .await?;
    harness
        .client
        .publish(
            verb_subject(RoutedVerb::RunCommand, "lease-fire-and-forget"),
            Bytes::new(),
        )
        .await?;

    let reply = harness
        .request(
            verb_subject(RoutedVerb::KillCommand, "lease-fire-and-forget"),
            b"{}".to_vec(),
        )
        .await?;
    assert_eq!(reply.payload.to_vec(), RoutedVerb::KillCommand.unsupported_reply()?);
    harness.stop().await
}

#[tokio::test]
async fn a_run_command_for_a_persistent_lease_is_evaluated_in_its_runtime() -> TestResult {
    let harness = Harness::native().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "globalThis.counter = 41;")?;
    let lease = Lease::mounted(dir.path(), "js").persistent().json();
    let granted = harness.dispatch("lease-live", lease).await?;
    assert_eq!(granted["worker_id"], json!("worker-under-test"));
    let subject = verb_subject(RoutedVerb::RunCommand, "lease-live");

    let eval = json!({ "lease_id": "lease-live", "command": ["eval", "counter += 1; console.log(counter);"], "timeout_seconds": 5 });
    let reply = harness.request(subject.clone(), serde_json::to_vec(&eval)?).await?;
    let evaluated: RunCommandResponse = serde_json::from_slice(&reply.payload)?;
    let garbage = harness.request(subject, b"not a command".to_vec()).await?;
    let refused: RunCommandResponse = serde_json::from_slice(&garbage.payload)?;

    assert!(matches!(
        evaluated,
        RunCommandResponse::Completed(output) if String::from_utf8_lossy(&output.stdout).contains("42")
    ));
    assert!(matches!(
        refused,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    assert_eq!(harness.v8.active_lease_count().await, 1);
    harness.stop().await
}

#[tokio::test]
async fn a_run_command_for_a_lease_without_a_runtime_is_denied() -> TestResult {
    let harness = Harness::native().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "1;")?;
    harness
        .dispatch("lease-oneshot", Lease::mounted(dir.path(), "js").json())
        .await?;
    let eval = json!({ "lease_id": "lease-oneshot", "command": ["eval", "1"], "timeout_seconds": 5 });

    let reply = harness
        .request(
            verb_subject(RoutedVerb::RunCommand, "lease-oneshot"),
            serde_json::to_vec(&eval)?,
        )
        .await?;

    let response: RunCommandResponse = serde_json::from_slice(&reply.payload)?;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    harness.stop().await
}

#[tokio::test]
async fn a_cancel_releases_a_native_lease_so_its_id_can_launch_again() -> TestResult {
    let harness = Harness::native().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "1;")?;
    let lease = LeaseId::new("lease-reused");
    harness
        .dispatch(lease.as_str(), Lease::mounted(dir.path(), "js").persistent().json())
        .await?;
    assert_eq!(harness.v8.active_lease_count().await, 1);

    harness
        .client
        .publish(provider_cancel(ProviderKind::V8Wasm, &lease), Bytes::new())
        .await?;
    let released = tokio::time::timeout(REPLY_WAIT, async {
        while harness.v8.active_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    assert!(released.is_ok());
    harness
        .dispatch(lease.as_str(), Lease::mounted(dir.path(), "js").json())
        .await?;
    harness.stop().await
}

#[tokio::test]
async fn a_provider_bound_to_another_worker_cannot_serve() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let v8 = Arc::new(V8WasmProvider::new(server.jetstream().await));
    v8.bind_worker(&WorkerId::new("someone-else")?)?;

    let result = run_worker(
        server.client().await,
        v8.clone(),
        v8,
        WorkerId::new("worker-under-test")?,
        CancellationToken::new(),
    )
    .await;

    assert!(matches!(result, Err(WorkerError::Bind(_))));
    Ok(())
}

#[tokio::test]
async fn stopping_the_worker_stops_the_guests_it_still_holds() -> TestResult {
    let harness = Harness::native().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "1;")?;
    harness
        .dispatch("lease-held", Lease::mounted(dir.path(), "js").persistent().json())
        .await?;
    let v8 = harness.v8.clone();

    harness.stop().await?;

    assert_eq!(v8.active_lease_count().await, 0);
    Ok(())
}
