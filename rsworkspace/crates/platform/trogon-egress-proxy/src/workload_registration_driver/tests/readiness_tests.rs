use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use super::*;
use crate::audit::{AuditEvent, AuditSink};
use crate::audit_error::AuditError;
use crate::audit_readiness::AuditReadiness;
use crate::live_policy::{KvPolicyConfig, KvPolicyDriver, KvSnapshot};
use crate::runtime_activity::RuntimeActivity;
use crate::{Readiness, ReadinessStatus};

struct HealthSink {
    status: Mutex<AuditReadiness>,
    hold: AtomicBool,
    panic: AtomicBool,
    entered: AtomicUsize,
    release: Notify,
}

impl Default for HealthSink {
    fn default() -> Self {
        Self {
            status: Mutex::new(AuditReadiness::Ready),
            hold: AtomicBool::new(false),
            panic: AtomicBool::new(false),
            entered: AtomicUsize::new(0),
            release: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl AuditSink for HealthSink {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }
    async fn readiness(&self) -> AuditReadiness {
        self.entered.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic.load(Ordering::SeqCst), "fixture audit readiness panic");
        if self.hold.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        *self.status.lock().unwrap()
    }
}

struct Ready {
    readiness: Readiness,
    _policy: KvPolicyDriver,
    _activity: RuntimeActivity,
}

async fn ready(fixture: &Fixture, audit: Arc<HealthSink>) -> Ready {
    let policy = KvPolicyConfig::new(
        "POLICIES",
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::MIN,
    )
    .unwrap();
    let store = fixture
        .context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: policy.bucket().into(),
            max_value_size: REGISTRATION_MAX_BYTES as i32,
            max_bytes: LIVE_POLICY_MAX_STORE_BYTES,
            max_age: Duration::from_secs(325),
            ..Default::default()
        })
        .await
        .unwrap();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let document = serde_json::json!({"tenant":"acme", "project":"payments", "incarnation":1,
        "generation":1, "issued_at":now, "valid_until":now+30, "policy":""});
    store
        .put(policy.key(), serde_json::to_vec(&document).unwrap().into())
        .await
        .unwrap();
    let (snapshot, driver) = KvSnapshot::open(fixture.client.clone(), policy).await.unwrap();
    let readiness = Readiness::new(Some(fixture.identity.clone()), Some(snapshot), audit);
    let activity = readiness.running(CancellationToken::new());
    Ready {
        readiness,
        _policy: driver,
        _activity: activity,
    }
}

#[tokio::test]
async fn readiness_preserves_every_audit_refusal_and_recovers_only_after_current_authority() {
    let fixture = Fixture::new().await;
    let audit = Arc::new(HealthSink::default());
    let ready = ready(&fixture, audit.clone()).await;
    for (state, expected) in [
        (AuditReadiness::Degraded, ReadinessStatus::AuditDegraded),
        (AuditReadiness::Overloaded, ReadinessStatus::AuditOverloaded),
        (AuditReadiness::TimedOut, ReadinessStatus::AuditTimedOut),
        (AuditReadiness::Unavailable, ReadinessStatus::AuditUnavailable),
        (AuditReadiness::Ready, ReadinessStatus::Ready),
    ] {
        *audit.status.lock().unwrap() = state;
        assert_eq!(ready.readiness.probe().await, expected);
    }
    fixture.identity.withdraw_registration();
    assert_eq!(ready.readiness.probe().await, ReadinessStatus::IdentityUnavailable);
    assert_eq!(
        audit.entered.load(Ordering::SeqCst),
        5,
        "missing authority cannot probe audit or report ready"
    );
    fixture.stop().await;
}

async fn response(mut stream: TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    match tokio::time::timeout(WAIT, stream.read_to_end(&mut bytes))
        .await
        .unwrap()
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(error) => panic!("HTTP response failed: {error}"),
    }
    bytes
}

#[tokio::test]
async fn public_http_probe_capacity_refuses_excess_work_and_recovers_after_held_audit_settles() {
    let fixture = Fixture::new().await;
    let audit = Arc::new(HealthSink::default());
    let ready = ready(&fixture, audit.clone()).await;
    audit.hold.store(true, Ordering::SeqCst);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(ready.readiness.clone().serve(listener, shutdown.clone()));
    let mut held = Vec::new();
    for _ in 0..crate::constants::READINESS_HTTP_CONNECTIONS {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
        held.push(stream);
    }
    tokio::time::timeout(WAIT, async {
        while audit.entered.load(Ordering::SeqCst) != held.len() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let extra = TcpStream::connect(address).await.unwrap();
    assert!(response(extra).await.is_empty());
    assert_eq!(
        audit.entered.load(Ordering::SeqCst),
        held.len(),
        "overflow cannot dispatch another audit probe"
    );
    audit.hold.store(false, Ordering::SeqCst);
    audit.release.notify_waiters();
    for stream in held {
        assert!(response(stream).await.starts_with(b"HTTP/1.1 200 OK"));
    }
    let mut recovered = TcpStream::connect(address).await.unwrap();
    recovered.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
    assert!(response(recovered).await.starts_with(b"HTTP/1.1 200 OK"));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_panicking_http_audit_probe_closes_only_its_worker_and_the_listener_recovers() {
    let fixture = Fixture::new().await;
    let audit = Arc::new(HealthSink::default());
    let ready = ready(&fixture, audit.clone()).await;
    audit.panic.store(true, Ordering::SeqCst);
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(ready.readiness.clone().serve(listener, shutdown.clone()));
    let mut failed = TcpStream::connect(address).await.unwrap();
    failed.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
    assert!(response(failed).await.is_empty());
    tokio::time::timeout(WAIT, async {
        while !events
            .events()
            .iter()
            .any(|event| event.message() == Some("HTTP readiness worker ended abnormally"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    audit.panic.store(false, Ordering::SeqCst);
    let mut recovered = TcpStream::connect(address).await.unwrap();
    recovered.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
    assert!(response(recovered).await.starts_with(b"HTTP/1.1 200 OK"));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
