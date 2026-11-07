use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};

use crate::AgentId;
use crate::audit::{AuditEvent, AuditSink, Decision};
use crate::audit_error::AuditError;
use crate::connection::Proxy;
use crate::destination::Destination;
use crate::fault::Fault;
use crate::limits::Limits;
use crate::live_policy::{KvPolicyConfig, KvSnapshot};
use crate::peer_identity::PeerIdentity;
use crate::policy_revision::PolicyRevision;
use crate::policy_snapshot::{PolicySnapshot, PolicyStatistics, SnapshotVerdict};
use crate::resolver::SystemResolver;
use async_nats::jetstream::kv;
use tokio_util::sync::CancellationToken;
use trogon_audit::AuditName;
use trogon_nats::NatsToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::env::InMemoryEnv;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use crate::constants::{LIVE_POLICY_TEST_POLL as POLL, LIVE_POLICY_TEST_WAIT as WAIT, LIVE_POLICY_TUNNEL_TEST_WAIT};

fn envelope(generation: u64, agent: &str) -> Vec<u8> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    serde_json::to_vec(&serde_json::json!({
        "tenant": "acme",
        "project": "payments",
        "incarnation": 1,
        "generation": generation,
        "issued_at": now,
        "valid_until": now + 30,
        "policy": format!("[agents.{agent}]\nallow = [\"api.example.com:443\"]\n")
    }))
    .unwrap()
}

#[tokio::test]
async fn a_complete_live_snapshot_replaces_the_previously_permitted_agent() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(kv::Config {
            bucket: "egress_policy".into(),
            max_value_size: 256 * 1024,
            max_bytes: 1024 * 1024,
            max_age: std::time::Duration::from_secs(325),
            ..kv::Config::default()
        })
        .await
        .unwrap();
    store.put(config().key(), envelope(1, "agent-1").into()).await.unwrap();
    let (snapshot, driver) = KvSnapshot::open(client, config()).await.unwrap();
    let agent = AgentId::new("agent-1").unwrap();
    let destination: Destination = "api.example.com:443".parse().unwrap();
    assert_eq!(
        snapshot.lookup(&agent, &destination, SystemTime::now()),
        SnapshotVerdict::Permitted
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    store.put(config().key(), envelope(2, "agent-2").into()).await.unwrap();
    let changed = tokio::time::timeout(WAIT, async {
        while snapshot.lookup(&agent, &destination, SystemTime::now()) != SnapshotVerdict::NotPermitted {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    assert!(changed.is_ok(), "a complete replacement must remove the prior permit");
    assert_eq!(
        snapshot.lookup(&AgentId::new("agent-2").unwrap(), &destination, SystemTime::now()),
        SnapshotVerdict::Permitted
    );
    shutdown.cancel();
    task.await.unwrap();
}

fn config() -> KvPolicyConfig {
    KvPolicyConfig::new(
        "egress_policy",
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap()
}

struct Fixture {
    _server: Option<JetStreamTestServer>,
    client: async_nats::Client,
    store: kv::Store,
    snapshot: std::sync::Arc<KvSnapshot>,
    shutdown: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    async fn start() -> Self {
        Self::with_document(|| envelope(1, "agent-1")).await
    }

    async fn with_document(document: impl FnOnce() -> Vec<u8>) -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let store = async_nats::jetstream::new(server.client().await)
            .create_key_value(bucket_config())
            .await
            .unwrap();
        store.put(config().key(), document().into()).await.unwrap();
        let (snapshot, driver) = KvSnapshot::open(client.clone(), config()).await.unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        Self {
            _server: Some(server),
            client,
            store,
            snapshot,
            shutdown,
            task: Some(task),
        }
    }

    async fn put(&self, document: Vec<u8>) {
        self.store.put(config().key(), document.into()).await.unwrap();
    }

    async fn verdict(&self, expected: SnapshotVerdict) {
        tokio::time::timeout(WAIT, async {
            loop {
                if current(&self.snapshot) == expected {
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await
        .expect("the live source must reach the expected verdict");
    }

    async fn generation(&self, generation: u64) {
        tokio::time::timeout(WAIT, async {
            loop {
                if self
                    .snapshot
                    .observe(
                        &AgentId::new("agent-1").unwrap(),
                        &"api.example.com:443".parse().unwrap(),
                        SystemTime::now(),
                    )
                    .revision
                    .is_some_and(|revision| revision.generation.get() == generation)
                {
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await
        .expect("the live source must accept the new generation");
    }

    async fn stop(&mut self) {
        self.shutdown.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn bucket_config() -> kv::Config {
    kv::Config {
        bucket: "egress_policy".into(),
        max_value_size: 256 * 1024,
        max_bytes: 1024 * 1024,
        max_age: std::time::Duration::from_secs(325),
        ..kv::Config::default()
    }
}

fn current(snapshot: &KvSnapshot) -> SnapshotVerdict {
    snapshot.lookup(
        &AgentId::new("agent-1").unwrap(),
        &"api.example.com:443".parse().unwrap(),
        SystemTime::now(),
    )
}

fn altered(document: Vec<u8>, field: &str, value: serde_json::Value) -> Vec<u8> {
    let mut document: serde_json::Value = serde_json::from_slice(&document).unwrap();
    document[field] = value;
    serde_json::to_vec(&document).unwrap()
}

#[tokio::test]
async fn a_complete_empty_snapshot_withdraws_every_permit() {
    let fixture = Fixture::start().await;
    fixture
        .put(altered(envelope(2, "agent-1"), "policy", serde_json::json!("")))
        .await;
    fixture.verdict(SnapshotVerdict::NotPermitted).await;
    fixture.generation(2).await;
}

#[tokio::test]
async fn removal_and_invalid_authority_cannot_reactivate_an_earlier_generation() {
    let fixture = Fixture::start().await;
    let first = envelope(1, "agent-1");
    fixture.store.delete(config().key()).await.unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    fixture.put(first.clone()).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
    fixture.put(envelope(2, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
    fixture.store.purge(config().key()).await.unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    fixture.put(envelope(2, "agent-1")).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
    fixture.put(envelope(3, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
    fixture
        .put(altered(envelope(4, "agent-1"), "tenant", serde_json::json!("other")))
        .await;
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    fixture.put(first).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
    fixture.put(envelope(5, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
}

#[tokio::test]
async fn a_replayed_or_equivocating_producer_revision_refuses_authority() {
    let fixture = Fixture::start().await;
    fixture.put(envelope(2, "agent-2")).await;
    fixture.verdict(SnapshotVerdict::NotPermitted).await;
    fixture.put(envelope(1, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    fixture.put(envelope(3, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
    fixture.put(envelope(3, "agent-2")).await;
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    fixture.put(envelope(4, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
}

#[tokio::test]
async fn a_reconnect_requires_a_fresh_producer_generation() {
    let fixture = Fixture::start().await;
    let before = fixture.client.server_info().client_id;
    fixture.client.force_reconnect().await.unwrap();
    tokio::time::timeout(WAIT, async {
        while fixture.client.connection_state() != async_nats::connection::State::Connected
            || fixture.client.server_info().client_id == before
        {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
    fixture.put(envelope(1, "agent-1")).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
    fixture.put(envelope(2, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
}

#[tokio::test]
async fn a_duplicate_cannot_extend_the_monotonic_producer_deadline() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(bucket_config())
        .await
        .unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let document = altered(envelope(1, "agent-1"), "valid_until", serde_json::json!(now + 2));
    store.put(config().key(), document.clone().into()).await.unwrap();
    let (snapshot, driver) = KvSnapshot::open(client, config()).await.unwrap();
    assert_eq!(current(&snapshot), SnapshotVerdict::Permitted);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    assert_eq!(current(&snapshot), SnapshotVerdict::Unavailable);
    tokio::time::resume();
    store.put(config().key(), document.into()).await.unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(current(&snapshot), SnapshotVerdict::Unavailable);
    store.put(config().key(), envelope(2, "agent-1").into()).await.unwrap();
    tokio::time::timeout(WAIT, async {
        while current(&snapshot) != SnapshotVerdict::Permitted {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    task.await.unwrap();
    assert_eq!(current(&snapshot), SnapshotVerdict::Unavailable);
}

#[tokio::test]
async fn a_recreated_stream_is_not_automatically_adopted() {
    let fixture = Fixture::start().await;
    let context = async_nats::jetstream::new(fixture._server.as_ref().unwrap().client().await);
    context.delete_key_value(config().bucket()).await.unwrap();
    let store = context.create_key_value(bucket_config()).await.unwrap();
    store.put(config().key(), envelope(2, "agent-1").into()).await.unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
}

#[tokio::test]
async fn stopping_the_driver_withdraws_its_current_authority() {
    let mut fixture = Fixture::start().await;
    fixture.stop().await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
}

#[test]
fn scoped_keys_and_source_names_are_bounded_and_unambiguous() {
    assert_eq!(config().key(), "t61636d65.p7061796d656e7473");
    assert!(
        KvPolicyConfig::new(
            "bad.bucket",
            AuditName::new("acme").unwrap(),
            NatsToken::new("payments").unwrap(),
            NonZeroU64::new(1).unwrap()
        )
        .is_err()
    );
    let maximum = KvPolicyConfig::new(
        &"a".repeat(64),
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::new(u64::MAX).unwrap(),
    )
    .unwrap();
    let source = crate::policy_source::PolicySource::NatsKv {
        bucket: maximum.bucket().to_owned(),
        incarnation: NonZeroU64::new(u64::MAX).unwrap(),
    };
    assert!(AuditName::new(source.audit_name()).is_ok());
}

#[derive(Clone, Copy)]
enum ExpectedError {
    Scope,
    Incarnation,
    Inactive,
    Lease,
    Envelope,
    Bounds,
    Policy,
}

#[tokio::test]
async fn opening_live_authority_refuses_wrong_scope_invalid_time_and_unbounded_content() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(bucket_config())
        .await
        .unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let base = envelope(1, "agent-1");
    let too_many_agents = (0..1025)
        .map(|i| format!("[agents.a{i}]\nallow=[]\n"))
        .collect::<String>();
    let too_many_permits = (0..4097)
        .map(|i| format!("\"a{i}.example.com:443\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut duplicate = base.clone();
    duplicate.pop();
    duplicate.extend_from_slice(b",\"generation\":2}");
    let cases = [
        (
            altered(base.clone(), "tenant", serde_json::json!("other")),
            ExpectedError::Scope,
        ),
        (
            altered(base.clone(), "project", serde_json::json!("other")),
            ExpectedError::Scope,
        ),
        (
            altered(base.clone(), "incarnation", serde_json::json!(2)),
            ExpectedError::Incarnation,
        ),
        (
            altered(
                altered(base.clone(), "issued_at", serde_json::json!(now + 60)),
                "valid_until",
                serde_json::json!(now + 90),
            ),
            ExpectedError::Inactive,
        ),
        (
            altered(
                altered(base.clone(), "issued_at", serde_json::json!(now - 30)),
                "valid_until",
                serde_json::json!(now),
            ),
            ExpectedError::Inactive,
        ),
        (
            altered(base.clone(), "valid_until", serde_json::json!(now + 326)),
            ExpectedError::Lease,
        ),
        (
            altered(base.clone(), "generation", serde_json::json!(0)),
            ExpectedError::Envelope,
        ),
        (
            altered(base.clone(), "issued_at", serde_json::json!(0.5)),
            ExpectedError::Envelope,
        ),
        (
            altered(base.clone(), "extra", serde_json::json!(true)),
            ExpectedError::Envelope,
        ),
        (
            altered(base.clone(), "policy", serde_json::json!(too_many_agents)),
            ExpectedError::Bounds,
        ),
        (
            altered(
                base.clone(),
                "policy",
                serde_json::json!(format!("[agents.agent-1]\nallow=[{too_many_permits}]\n")),
            ),
            ExpectedError::Bounds,
        ),
        (
            altered(
                base,
                "policy",
                serde_json::json!("[agents.a]\nallow=[\"*.example.com:443\"]\n"),
            ),
            ExpectedError::Policy,
        ),
        (duplicate, ExpectedError::Envelope),
    ];
    for (index, (document, expected)) in cases.into_iter().enumerate() {
        store.put(config().key(), document.into()).await.unwrap();
        let error = KvSnapshot::open(client.clone(), config())
            .await
            .err()
            .expect("invalid authority must fail startup");
        assert!(
            matches!(
                (&error, expected),
                (crate::live_policy::LivePolicyError::Scope, ExpectedError::Scope)
                    | (
                        crate::live_policy::LivePolicyError::Incarnation,
                        ExpectedError::Incarnation
                    )
                    | (crate::live_policy::LivePolicyError::Inactive, ExpectedError::Inactive)
                    | (crate::live_policy::LivePolicyError::Lease, ExpectedError::Lease)
                    | (crate::live_policy::LivePolicyError::Envelope, ExpectedError::Envelope)
                    | (crate::live_policy::LivePolicyError::Bounds, ExpectedError::Bounds)
                    | (crate::live_policy::LivePolicyError::Policy, ExpectedError::Policy)
            ),
            "case {index}: {}",
            error.reason()
        );
    }
}

#[tokio::test]
async fn an_unbounded_or_missing_bucket_cannot_supply_live_policy() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    assert!(matches!(
        KvSnapshot::open(client.clone(), config()).await.err().unwrap(),
        crate::live_policy::LivePolicyError::Bucket(_)
    ));
    let context = async_nats::jetstream::new(client.clone());
    let store = context
        .create_key_value(kv::Config {
            bucket: "egress_policy".into(),
            ..kv::Config::default()
        })
        .await
        .unwrap();
    store.put(config().key(), envelope(1, "agent-1").into()).await.unwrap();
    assert!(matches!(
        KvSnapshot::open(client, config()).await.err().unwrap(),
        crate::live_policy::LivePolicyError::BucketConfiguration
    ));
}

struct BlockingAudit {
    entered: mpsc::Sender<Option<PolicyRevision>>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl AuditSink for BlockingAudit {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        if event.decision == Decision::Permitted {
            self.entered.send(event.policy_revision).await.unwrap();
            self.release.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}

struct AllowAudit;

#[async_trait]
impl AuditSink for AllowAudit {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }
}

fn ip_policy(port: u16) -> String {
    format!("[agents.agent-1]\nallow=[\"127.0.0.1:{port}\"]\n")
}

fn proxy(snapshot: Arc<KvSnapshot>, audit: Arc<dyn AuditSink>) -> Proxy {
    Proxy::new(
        snapshot,
        Arc::new(SystemResolver),
        audit,
        Limits::default(),
        Default::default(),
    )
}

async fn serve(proxy: Proxy, port: u16) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<()>) {
    let (mut client, server) = tokio::io::duplex(4096);
    let peer = PeerIdentity::for_test(AgentId::new("agent-1").unwrap(), std::time::Duration::from_secs(60));
    let task = tokio::spawn(async move { proxy.serve(server, &peer).await });
    client
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    (client, task)
}

async fn read_all(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
    let mut response = Vec::new();
    tokio::time::timeout(LIVE_POLICY_TUNNEL_TEST_WAIT, client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn a_new_policy_generation_during_allow_audit_cannot_reuse_the_old_acknowledgement() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let received = tokio::spawn(async move {
        let (mut upstream, _) = upstream.accept().await.unwrap();
        let mut bytes = Vec::new();
        upstream.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let document = altered(envelope(1, "agent-1"), "policy", serde_json::json!(ip_policy(port)));
    let fixture = Fixture::with_document(|| document).await;
    let (entered, mut observed) = mpsc::channel(1);
    let release = Arc::new(Semaphore::new(0));
    let audit = Arc::new(BlockingAudit {
        entered,
        release: Arc::clone(&release),
    });
    let (mut client, task) = serve(proxy(Arc::clone(&fixture.snapshot), audit), port).await;
    let revision = tokio::time::timeout(WAIT, observed.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(revision.incarnation.get(), 1);
    assert_eq!(revision.generation.get(), 1);
    fixture
        .put(altered(
            envelope(2, "agent-1"),
            "policy",
            serde_json::json!(ip_policy(port)),
        ))
        .await;
    fixture.generation(2).await;
    release.add_permits(1);
    assert_eq!(read_all(&mut client).await, Fault::PolicyUnavailable.response());
    task.await.unwrap();
    assert!(
        received.await.unwrap().is_empty(),
        "stale audit acknowledgement cannot forward a byte"
    );
}

#[tokio::test]
async fn a_removed_live_permit_closes_an_existing_tunnel_and_refuses_a_new_connection() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let echoed = tokio::spawn(async move {
        let (mut upstream, _) = upstream.accept().await.unwrap();
        let mut received = Vec::new();
        let mut chunk = [0; 128];
        loop {
            let read = upstream.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..read]);
            upstream.write_all(&chunk[..read]).await.unwrap();
        }
        received
    });
    let fixture =
        Fixture::with_document(|| altered(envelope(1, "agent-1"), "policy", serde_json::json!(ip_policy(port)))).await;
    let (mut client, task) = serve(proxy(Arc::clone(&fixture.snapshot), Arc::new(AllowAudit)), port).await;
    let mut head = vec![0; crate::constants::CONNECTION_ESTABLISHED.len()];
    tokio::time::timeout(WAIT, client.read_exact(&mut head))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head, crate::constants::CONNECTION_ESTABLISHED);
    client.write_all(b"hello").await.unwrap();
    let mut echo = [0; 5];
    tokio::time::timeout(WAIT, client.read_exact(&mut echo))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&echo, b"hello");
    fixture
        .put(altered(envelope(2, "agent-1"), "policy", serde_json::json!("")))
        .await;
    fixture.generation(2).await;
    assert!(read_all(&mut client).await.is_empty());
    task.await.unwrap();
    assert_eq!(echoed.await.unwrap(), b"hello");
    let (mut next, task) = serve(proxy(Arc::clone(&fixture.snapshot), Arc::new(AllowAudit)), port).await;
    assert_eq!(read_all(&mut next).await, Fault::NoPermit.response());
    task.await.unwrap();
}

#[tokio::test]
async fn an_established_tunnel_ends_at_the_exact_monotonic_producer_deadline() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let (mut upstream, _) = upstream.accept().await.unwrap();
        let mut bytes = Vec::new();
        upstream.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let fixture = Fixture::with_document(|| {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        altered(
            altered(envelope(1, "agent-1"), "policy", serde_json::json!(ip_policy(port))),
            "valid_until",
            serde_json::json!(now + 2),
        )
    })
    .await;
    let (mut client, task) = serve(proxy(Arc::clone(&fixture.snapshot), Arc::new(AllowAudit)), port).await;
    let mut head = vec![0; crate::constants::CONNECTION_ESTABLISHED.len()];
    tokio::time::timeout(WAIT, client.read_exact(&mut head))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head, crate::constants::CONNECTION_ESTABLISHED);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let mut byte = [0];
    let closed = tokio::time::timeout(std::time::Duration::ZERO, client.read(&mut byte)).await;
    tokio::time::resume();
    assert!(
        matches!(closed, Ok(Ok(0))),
        "the two-second lease cannot survive to the five-second periodic check"
    );
    task.await.unwrap();
    assert!(held.await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_live_documents_cannot_escape_through_the_startup_error_chain() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(bucket_config())
        .await
        .unwrap();
    let opaque = format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
    let documents = [
        altered(envelope(1, "agent-1"), "generation", serde_json::json!(opaque)),
        altered(
            envelope(1, "agent-1"),
            "policy",
            serde_json::json!(format!("[agents.agent-1]\nallow=[\"{opaque}\"]\n")),
        ),
    ];
    for document in documents {
        store.put(config().key(), document.into()).await.unwrap();
        let error = KvSnapshot::open(client.clone(), config()).await.err().unwrap();
        assert!(matches!(
            error,
            crate::live_policy::LivePolicyError::Envelope | crate::live_policy::LivePolicyError::Policy
        ));
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[tokio::test]
async fn a_shorter_replacement_lease_wakes_an_already_established_tunnel() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let (mut upstream, _) = upstream.accept().await.unwrap();
        let mut bytes = Vec::new();
        upstream.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let fixture =
        Fixture::with_document(|| altered(envelope(1, "agent-1"), "policy", serde_json::json!(ip_policy(port)))).await;
    let (mut client, task) = serve(proxy(Arc::clone(&fixture.snapshot), Arc::new(AllowAudit)), port).await;
    let mut head = vec![0; crate::constants::CONNECTION_ESTABLISHED.len()];
    tokio::time::timeout(WAIT, client.read_exact(&mut head))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head, crate::constants::CONNECTION_ESTABLISHED);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    fixture
        .put(altered(
            altered(envelope(2, "agent-1"), "policy", serde_json::json!(ip_policy(port))),
            "valid_until",
            serde_json::json!(now + 2),
        ))
        .await;
    fixture.generation(2).await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let mut byte = [0];
    let closed = tokio::time::timeout(std::time::Duration::ZERO, client.read(&mut byte)).await;
    tokio::time::resume();
    assert!(
        matches!(closed, Ok(Ok(0))),
        "the replacement lease cannot wait for the old deadline or periodic check"
    );
    task.await.unwrap();
    assert!(held.await.unwrap().is_empty());
}

#[tokio::test]
async fn a_retrying_initial_connection_synchronizes_before_reading_live_authority() {
    let server = JetStreamTestServer::start().await;
    let store = server
        .jetstream()
        .await
        .create_key_value(bucket_config())
        .await
        .unwrap();
    store.put(config().key(), envelope(1, "agent-1").into()).await.unwrap();
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(server.address())
        .await
        .unwrap();
    let opened = KvSnapshot::open(client, config()).await;
    assert!(
        opened.is_ok(),
        "a ready broker must be synchronized before reading the connection identity"
    );
}

#[tokio::test]
async fn initial_connection_synchronization_has_a_finite_deadline() {
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("nats://127.0.0.1:1")
        .await
        .unwrap();
    let opened = tokio::time::timeout(WAIT, KvSnapshot::open(client, config()))
        .await
        .expect("initial synchronization must not wait indefinitely");
    assert!(matches!(
        opened.err().unwrap(),
        crate::live_policy::LivePolicyError::Timeout | crate::live_policy::LivePolicyError::Disconnected
    ));
}

async fn warning(events: &CapturedEvents, after: usize, expected: &str) {
    tokio::time::timeout(WAIT, async {
        loop {
            if events.events().iter().skip(after).any(|event| {
                event.message() == Some("live egress policy unavailable") && event.field("reason") == Some(expected)
            }) {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .expect("the closed operational failure reason must be emitted");
}

#[derive(Clone, Copy)]
enum RejectedUpdate {
    Scope,
    Incarnation,
    Envelope,
    Syntax,
    Agent,
    Destination,
    Bounds,
    Expired,
    Lease,
    Replay,
}

#[tokio::test]
async fn rejected_live_updates_report_closed_operational_reasons_and_recover_only_with_new_authority() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let fixture = Fixture::start().await;
    let mut generation = 2;
    for scenario in [
        RejectedUpdate::Scope,
        RejectedUpdate::Incarnation,
        RejectedUpdate::Envelope,
        RejectedUpdate::Syntax,
        RejectedUpdate::Agent,
        RejectedUpdate::Destination,
        RejectedUpdate::Bounds,
        RejectedUpdate::Expired,
        RejectedUpdate::Lease,
        RejectedUpdate::Replay,
    ] {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let base = envelope(generation, "agent-1");
        let (invalid, reason) = match scenario {
            RejectedUpdate::Scope => (altered(base, "tenant", serde_json::json!("other")), "scope"),
            RejectedUpdate::Incarnation => (
                altered(base, "incarnation", serde_json::json!(2)),
                "authority_incarnation",
            ),
            RejectedUpdate::Envelope => (altered(base, "issued_at", serde_json::json!(0.5)), "invalid_document"),
            RejectedUpdate::Syntax => (
                altered(base, "policy", serde_json::json!("[agents.agent-1")),
                "invalid_document",
            ),
            RejectedUpdate::Agent => (
                altered(base, "policy", serde_json::json!("[agents.\"\"]\nallow=[]\n")),
                "invalid_document",
            ),
            RejectedUpdate::Destination => (
                altered(
                    base,
                    "policy",
                    serde_json::json!("[agents.agent-1]\nallow=[\"*.example.com:443\"]\n"),
                ),
                "invalid_document",
            ),
            RejectedUpdate::Bounds => (
                altered(
                    base,
                    "policy",
                    serde_json::json!(
                        (0..1025)
                            .map(|i| format!("[agents.a{i}]\nallow=[]\n"))
                            .collect::<String>()
                    ),
                ),
                "resource_bounds",
            ),
            RejectedUpdate::Expired => (
                altered(
                    altered(base, "issued_at", serde_json::json!(now - 30)),
                    "valid_until",
                    serde_json::json!(now),
                ),
                "inactive",
            ),
            RejectedUpdate::Lease => (altered(base, "valid_until", serde_json::json!(now + 326)), "lease"),
            RejectedUpdate::Replay => (envelope(1, "agent-1"), "regression"),
        };
        let before = events.events().len();
        let observed = fixture.snapshot.observe(
            &AgentId::new("agent-1").unwrap(),
            &"api.example.com:443".parse().unwrap(),
            SystemTime::now(),
        );
        fixture.put(invalid).await;
        fixture.verdict(SnapshotVerdict::Unavailable).await;
        warning(&events, before, reason).await;
        assert!(
            !fixture
                .snapshot
                .revision_is_current(observed.revision, SystemTime::now())
        );
        assert_eq!(fixture.snapshot.statistics(), None);
        generation += 1;
        fixture.put(envelope(generation, "agent-1")).await;
        fixture.verdict(SnapshotVerdict::Permitted).await;
        assert_eq!(
            fixture.snapshot.statistics(),
            Some(PolicyStatistics { agents: 1, permits: 1 })
        );
        generation += 1;
    }
    fixture
        .put(altered(
            envelope(generation, "agent-1"),
            "policy",
            serde_json::json!(""),
        ))
        .await;
    fixture.verdict(SnapshotVerdict::NotPermitted).await;
    assert_eq!(
        fixture.snapshot.statistics(),
        Some(PolicyStatistics { agents: 0, permits: 0 })
    );
}

#[tokio::test]
async fn a_disconnected_broker_withdraws_policy_and_reports_its_connection_failure() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let mut fixture = Fixture::start().await;
    let before = events.events().len();
    drop(fixture._server.take());
    tokio::time::timeout(WAIT, async {
        while fixture.client.connection_state() == async_nats::connection::State::Connected {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .expect("the broker transport must become disconnected");
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    warning(&events, before, "disconnected").await;
    assert_eq!(fixture.snapshot.statistics(), None);
}

#[tokio::test]
async fn bucket_loss_configuration_drift_and_recreation_report_distinct_failures() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let fixture = Fixture::start().await;
    let context = fixture._server.as_ref().unwrap().jetstream().await;
    let mut invalid = bucket_config();
    invalid.max_bytes = 0;
    let before = events.events().len();
    context.update_key_value(invalid).await.unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    warning(&events, before, "bucket_configuration").await;
    context.update_key_value(bucket_config()).await.unwrap();
    fixture.put(envelope(2, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
    let before = events.events().len();
    context.delete_key_value(config().bucket()).await.unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    warning(&events, before, "transport").await;
    let before = events.events().len();
    let recreated = context.create_key_value(bucket_config()).await.unwrap();
    recreated
        .put(config().key(), envelope(3, "agent-1").into())
        .await
        .unwrap();
    warning(&events, before, "stream_incarnation").await;
    assert_eq!(current(&fixture.snapshot), SnapshotVerdict::Unavailable);
}

#[tokio::test]
async fn malformed_kv_operations_are_refused_and_reported_before_recovery() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let fixture = Fixture::start().await;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(crate::constants::KV_OPERATION_HEADER, "INVALID");
    let before = events.events().len();
    fixture
        ._server
        .as_ref()
        .unwrap()
        .jetstream()
        .await
        .publish_with_headers(
            format!("$KV.{}.{}", config().bucket(), config().key()),
            headers,
            envelope(2, "agent-1").into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    fixture.verdict(SnapshotVerdict::Unavailable).await;
    warning(&events, before, "removed").await;
    fixture.put(envelope(3, "agent-1")).await;
    fixture.verdict(SnapshotVerdict::Permitted).await;
}

#[test]
fn a_partial_client_certificate_cannot_enable_live_authority() {
    let env = InMemoryEnv::new();
    env.set("NATS_CERT", "client.pem");
    let config = trogon_nats::NatsConfig::from_env(&env);
    assert!(matches!(
        KvPolicyConfig::validate_transport(&config),
        Err(crate::live_policy::PolicyTransportError::TlsInvalid(
            trogon_nats::TlsMisconfiguration::IncompleteClientCertificate(
                trogon_nats::tls::IncompleteClientCertificate::MissingKey
            )
        ))
    ));
}
